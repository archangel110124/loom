# ADR 0085 — A lamp ray is the sun's ray with TMax at the lamp

- **Date:** 2026-09-06
- **Status:** **accepted** — third of the nine subsystems scoped as "what is
  missing for a full-blown game engine", at the general-engine bar.
- **Decision touched:** none of the locked table. No new pass, pipeline,
  descriptor, buffer or barrier. One overload in `scene.slang` and one call
  site. Extends ADR 0019 rather than replacing anything in it.
- **Human decisions this records:** ray-traced rather than shadow maps, and
  re-bless with before/after rather than defaulting the feature off.

## 1. What was there

One `Light`: a point light with a windowed inverse-square falloff. It lit
things and cast nothing. Only the sun cast, through `sunVisibility`.

The old comment at the call site gave the reason, and half of it was wrong:

> *"They are unshadowed: a shadowed point light needs a cube map or a ray per
> light per pixel, and the ray tracer here is aimed at the sun."*

The first clause is a true statement about shadow maps. The second is the
mistake: **the TLAS is built and traced every frame already**, for sun
visibility, ambient occlusion and reflections. A lamp ray is the sun's ray with
`TMax` at the lamp instead of the horizon. There was no machinery to add.

## 2. Decision

`pointLights` gains a `shadowBias` parameter. Non-zero traces one bounded
`anyHit` toward each light before accumulating it; zero traces nothing and is
byte-identical to the old behaviour.

**Bounded, not infinite, and that is the whole difference between the two
rays.** A sun ray asks *is anything in this direction*. A lamp ray asks *is
anything between us* — an unbounded one finds the wall behind the lamp and puts
every surface in shadow. `TMax` is `d - bias`, so geometry *at* the lamp cannot
shadow the lamp from itself.

**The test comes before the ray.** A surface facing away from a lamp
contributes nothing whether or not something blocks it, and tracing to discover
that is the most expensive way to multiply by zero. `ndotl <= 0` and a zeroed
attenuation both skip the trace.

**Surfaces only.** The volume and particle callers pass zero. A shadow ray per
light per smoke puff is thousands of traversals for a contribution already
softened by the medium — and the TLAS holds no smoke to be shadowed by anyway.

## 3. What it looks like

Twelve of the 62 references moved: `campfire`, `lanternhead`, `stoneyard`,
`homestead`, `proving_ground`, `mirrorpool`, `explosion`, `plume`, `plume_gale`,
`plume_roof`, `deeper_demo_squall`. Re-blessed after the human read
before/after pairs.

On `campfire` the two stones and the log now throw shadows away from the fire.
On `lanternhead` the brazier's light stops passing *through* the quay wall.
That second one is the point: what a campfire mostly does is light everything
near it, and it could always do that. What it could not do was put anything
*behind* something else, which is the entire vocabulary of a dark room with a
lantern in it.

## 4. Cost, and why no number is quoted

`lanternhead` at 1920x1080, forward pass, three runs each side:

    shadows off   2.552, 1.165, 1.489 ms
    shadows on    1.412, 1.449, 2.233 ms

**The spread swamps the difference, so there is no measurement here.** The
min-of-3 pair would read 1.165 against 1.412 and invite a "0.25 ms" claim; two
batches taken minutes apart are two draws from a drifting distribution, and this
project has already withdrawn one cost claim made exactly that way (ADR 0081).
What can be said without a better instrument: the cost is **one bounded ray per
light per lit pixel**, `lanternhead` has three lights, and it is under the noise
floor of this measurement. An honest number needs interleaved runs on an idle
box.

## 5. Limits, named rather than discovered later

- **The TLAS holds meshes only.** Grass, water, rain, fire and smoke are
  generated from `SV_VertexID` and cannot be hit, so none of them casts a lamp
  shadow. A brazier's flame lights the quay and casts nothing — the same
  limitation ADR 0019 records for reflections, in a second place now.
- **Hard shadows.** `sunVisibility` samples a 2° disc across four rays for a
  penumbra; a lamp gets one ray and a hard edge. The soft version is the same
  code with the light's radius in place of the sun's angle, and it is not built
  because nothing has asked for a soft lamp yet.
- **Reflections show unshadowed lamps.** The reflection-hit path still calls the
  unshadowed overload, so a lamp shadow visible directly is absent in the water
  reflecting it. Consistent would mean a ray per light per reflection sample.
## 6. The ablation row, which this feature could have shipped without

**A lamp that lights everything near it and casts nothing looks plausible.** The
absence is only obvious beside the presence — which is precisely what a
reference image cannot report, because it records the absence and passes for
ever. That is the failure `cargo xtask ablate` exists for, so lamp shadows get a
row rather than a note saying they deserve one.

    campfire   lamp_shadow   3.069%   floor 1.500%   ok

`campfire` rather than `lanternhead`, which moves more of its frame: most of
lanternhead's movement is a quay wall changing brightness, while campfire is two
stones and a log on open ground, so what the ablation removes is unmistakably
the shadows themselves.

Three coordinated edits, as CLAUDE.md warns — `ABLATIONS` and its constant in
`ablate.rs`, `LOOM_ABLATE_LAMP_SHADOW` in `scene.slang`, and the `ABLATE` table
in `xtask` — and nothing checks that they agree. The array length is a fourth
place, and the floor is a **fraction**: `1.5` reads as 150% and fails every run,
which is how the first attempt at this row failed.
