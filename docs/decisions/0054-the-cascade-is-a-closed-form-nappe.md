# ADR 0054 — The cascade is a closed-form nappe

- **Date:** 2026-08-18
- **Status:** **proposed** — needs human approval before merge.
- **Decisions touched:** none locked. It adds a component and a slice of the
  water draw; it does **not** use ADR 0053's cinematic tier and needs nothing
  relaxed. It extends ADR 0006's generator rule to a third module and ADR 0019's
  "the TLAS holds meshes only" consequence to a third generated surface.

## Context

A waterfall is one of the two or three effects a viewer uses to decide whether
an engine's water is any good, and it is the one Loom had no answer for at all.
The obvious implementations are all bad in the same way: a scrolling card is a
texture with a speed knob, a particle curtain is thousands of sprites with an
overdraw problem and no silhouette, and a fluid solver is ADR 0053's tier —
GPU state, a readback, a per-scene reproducibility cost — for an effect that
does not need any of it.

**Free-fall hydraulics is a closed form.** Everything a renderer or a force
wants about the sheet leaving a weir crest is algebra in one authored number,
the discharge per metre of lip `q`. That is the same shape as the rest of this
project's water: `sample_water` is a closed form, `loom_water::foam`'s trail is
a closed form, and both are cheap, deterministic and CPU-readable precisely
because nothing about them is stepped.

## Decision

**A `Cascade` component draws a falling sheet whose entire geometry and shading
come from `loom_water::nappe`, a closed form generated into Slang by
`build.rs`.** No state, no readback, no tier, no second pass.

### 1. The hydraulics, and what each line is for

With `g = 9.81`, `q` the discharge per metre of lip, and `Z` the depth below it:

```
y_c   = (q²/g)^⅓                    critical depth
y_b   = 0.715 · y_c                 depth AT the brink        (Rouse 1936)
V_i   = q / y_b                     exit velocity → Fr = 1.65 for every q
X(Z)  = V_i √(2Z/g)                 horizontal throw          (ballistic)
V_j(Z)= √(V_i² + 2gZ)               jet velocity
D_j(Z)= y_b · V_i / V_j(Z)          WATER thickness           (continuity)
D_out = y_b + 2·δ_out·Z             OUTER thickness           (Ervine & Falvey)
aer(Z)= 1 − D_j / D_out             air fraction — why it is white
Z_b   = (y_b·V_i / (2·δ_in·√(2g)))^⅔   break-up depth         (Bollaert 2004)
```

Two things are worth stating because they are not obvious from the algebra.
`Fr = 1.65` at the brink for *every* discharge is a consequence of Rouse's
0.715, not a second coefficient — which is why a unit test asserts it rather
than a table. And a waterfall is white because of `aer`, which is a *ratio of
two thicknesses*, not because of a colour anywhere in the shader.

### 2. The two δ's are authored, and the derivation was attempted and abandoned

Bollaert gives `δ_in ≈ 0.38·Tu` and, separately, half-angles measured on
prototype falls. **They do not reconcile.** The quoted angles imply inner
spreads several times what `0.38·Tu` yields at any plausible turbulence
intensity, and five independent attempts in the design pass to derive one from
the other failed by different amounts each time.

So `spread` (`δ_out`) and `breakup` (`δ_in`) are authored knobs, with the
literature's ranges as their schema bounds — `0.015..0.04` and `0.005..0.01` —
and the middle of each as the default. **Writing down a derivation here would
have been fabricating one**, and a fabricated constant is worse than an honest
knob because nobody re-examines it.

### 3. The discrimination, which is what the closed form buys

| | `q` | `Z_b` | fall | at the pool | reads as |
| --- | --- | --- | --- | --- | --- |
| `spout.loom` (image 60) | 0.10 | **1.31 m** | 2 m | `Z/Z_b = 1.53` | stringy last third |
| `cascade.loom` (image 62) | 2.0 | **9.7 m** | 6 m | `Z/Z_b = 0.62` | white, coherent |

Both numbers are unit-tested. **One authored number moves the picture between
the two reference photographs**, and there is no look knob in either scene doing
any of that work — which is the entire argument for hydraulics over a shader
with a break-up slider.

### 4. Where it is drawn, and why there is no second pipeline

The strip rides in the **water draw**, past the surface's rings, branching on
`SV_VertexID` in `waterVertexMain`. It wants the same rasterisation state, the
same multisampled attachments, the same set-3 background textures, and it must
be depth-tested against the pool it pours into. A separate pass would be a
second pipeline, a second bind and a second entry point kept in step for 5,376
vertices.

That placement is also what buys it, for free: Fresnel against the same sky the
sea reflects, the same refracted background sample, 4x MSAA on its silhouette,
and no sort and no blend — the sheet is opaque geometry that *lerps* toward the
background, exactly as the water surface already does.

**In a scene with no cascade the strip collapses to a point**, the same trick
the covered centre of a water ring uses, so every scene in the repository draws
5,376 vertex-shader invocations that emit nothing and is otherwise unchanged.

### 5. What the shader adds on top, and it is labelled

Two things are presentation and are not pretending otherwise:

- **The streaks.** `loom_value_noise` — ADR 0006's frozen field noise, never a
  hand-rolled hash — evaluated in *(position across the lip, departure time)*.
  Departure time is `t − (V_j − V_i)/g`, which is exact, so the pattern is
  nailed to the **water** rather than to the air and stretches as the sheet
  accelerates with no scroll speed authored anywhere.
- **The lip boil**, `0.7·exp(−Z/0.3)`. In image 60 the whitest band of the whole
  fall is at the lip, where `aer` is by construction zero. What is white there
  is exit turbulence — the channel is already broken up before it arrives — and
  no coefficient in a free-fall model stands in for it. Two numbers picked by
  eye, and the source says so at the site.

### 6. Stated consequence: the fall is not in the pool's reflection

**The TLAS holds meshes only.** The nappe is `SV_VertexID` geometry with no
vertex buffer, so it cannot be an acceleration-structure instance and cannot be
hit by ADR 0019's traced reflection ray — the same standing consequence grass,
water, rain, fire and smoke already carry. `cascade.loom`'s pool reflects the
canyon wall and not the waterfall in front of it.

**This was decided, not discovered.** All three judges viewed reference image
62, where the base of the fall is broken up by mist and the reflection zone
under it is nearly featureless anyway, and ruled it acceptable.

**The revisit trigger** is a scene whose composition depends on the reflected
fall — a still pool seen from low down with the cascade filling it. The fix is
to make the sheet an `Object` with a real vertex buffer so it enters the TLAS,
which costs a **BLAS refit per frame** because the strip's vertices move every
tick: at 32x28 quads that is 1,792 triangles, which is small, but it introduces
the first per-frame acceleration-structure rebuild in the engine and needs its
own measurement. Do not do it speculatively.

### 7. What was deliberately left out

- **Lip auto-detection from the D8 flow bake.** `loom_water::flow` already
  routes water downhill and knows where a channel goes over an edge, so a
  cascade could in principle place itself. It is out of scope here and it is a
  genuinely good idea; it needs an answer to how an author overrides it.
- **A force.** The nappe is *force-capable* — that is why it lives in
  `loom_water`, which cannot import `ash`, and why its Slang twin goes through
  the generator and the agreement test rather than being hand-written. But
  nothing produces a force yet: `V_j` and `D_j` at a point are exactly what a
  drag term on a character walking under the fall would need, and adding it is
  a few lines whenever a scene wants one. Building it now would be a knob wired
  to nothing.
- **Side contraction.** Real nappes pull in at their ends. It is another
  coefficient with the same reconciliation problem as the δ's and it is worth
  perhaps two per cent of the picture.
- **More than one cascade per scene.** The environment buffer carries one lip;
  a second is refused at load with the reason, rather than silently not drawn.

## Consequences

The engine gains a waterfall that is a pure function of `(scene, tick)`, is
readable on the CPU by anything that wants it, costs nothing in a scene that
does not author one, and is authored by one number that carries real physical
meaning. It gains two new golden references, `spout` and `cascade`, which exist
as a pair because the feature *is* the discrimination between them.

**The honest risk** is the streak noise. It is a fragment-shader function
evaluated per pixel, so 4x MSAA does not touch it, and at 26 cycles across a
lip its second octave is a few pixels wide at 480x300. Sub-pixel shading detail
is the failure class P2 spent a phase on. `cargo xtask shimmer` at the authored
camera is the instrument, and it has not been run on these two scenes.
