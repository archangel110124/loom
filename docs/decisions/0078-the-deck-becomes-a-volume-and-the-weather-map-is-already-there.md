# ADR 0078 — The deck becomes a volume, and the weather map was already there

- **Date:** 2026-09-05
- **Status:** **proposed.**
- **Decision touched:** none of CLAUDE.md's locked decisions. No new pass, no post-process,
  no new resource type, no new dependency, no GPU state. **Governed by ADR 0045:** this is
  rendering-only, produces no force, is unreadable by `loom sim --assert` and by `rhai`,
  and adds no state — a frame stays a pure function of (scene, tick), so it adds no
  obligation to `cargo xtask repeat`.
- **Amends ADR 0015**, whose technique conclusion reads *"sky shader, not particles, not
  volumetric"*. **The first two clauses stand unchanged; the third is overturned.** Clouds
  stay in the sky shader and stay out of the particle system for exactly the reasons 0015
  gives, and this ADR does not reopen either. Only "not volumetric" moves.
- **Amends ADR 0016 §"What it does not do"**, first sentence: *"It does not make clouds
  volumetric — ADR 0015's technique conclusion stands, and its limits with it: no parallax,
  cannot be flown through, no god rays, no ground shadows without a separate feature."*
  Parallax is delivered here. **Cannot be flown through, god rays and ground shadows all
  still stand** — see §6.
- **Called for by ADR 0015 by name:** *"If it is ever wanted, it deserves its own ADR
  rather than arriving as a cloud detail."* This is that ADR.

---

## 1. Context

The clouds are a flat projection and they read as one.

`scene.slang:565` is the whole of it:

```hlsl
float3 cloudPlane(float3 dir) {
    const float DECK_HEIGHT = 900.0;
    float2 plane = dir.xz / max(dir.y, 0.10) * DECK_HEIGHT;
    return float3(plane.x, 0.0, plane.y) + push.environment[0].eye.xyz * float3(1, 0, 1);
}
```

A view direction is divided by its own elevation and multiplied by a constant altitude.
That is a texture pasted onto the inside of a hemisphere. It has no thickness, so it has
no parallax, no silhouette, no underside, and no self-shadowing beyond the single sunward
tap at `scene.slang:659` that ADR 0015 correctly described as *"cheap fake self-shadowing"*.

ADR 0015 deferred the alternative and gave two reasons. Both have expired:

> Deferred for two reasons. It is expensive in exactly the way this engine has no budget
> model for yet, and it is a full-screen effect, so it would sit against ADR 0010's
> post-process boundary.

**The second reason was wrong even when written, and is plainly wrong now.** `skyColor` is
already evaluated per pixel across every background fragment. Marching inside it adds no
pass, no target and no pipeline, so it never touches ADR 0010's boundary. Nothing about
raymarching a slab requires a full-screen pass; Nubis needs one because Nubis wants
half-resolution and temporal reprojection, which are optimisations, not the technique.

**The first reason is real and is answered by measuring, not by pre-buying.** §5 says how.

### What changed since 0015 was written

Three things, and they are the reason this is a much smaller build than 0015 assumed.

1. **The engine already marches volumes per fragment.** ADR 0020 put a line-integral flame
   in `scene.slang` and ADR 0050 extended it to soot. `SMOKE_STEPS = 20`, an early-out at
   `SMOKE_T_MIN = 0.02`, a Henyey–Greenstein phase at `smokePhase` (`scene.slang:4413`),
   Beer–Lambert in four separate places. A marched cloud deck is not a new *class* of thing
   here. It is a bigger instance of one that ships.

2. **`clouds_at` is already a weather map.** This is the load-bearing observation of this
   ADR and §2 is entirely about it.

3. **`loom_value_noise(float3 p)` is already generated, frozen and CPU-identical.**
   `assets/shaders/generated/fields.slang:26`, with the generator's own header on it: *"one
   implementation written twice, and `field_agree` asserts they are exactly equal."* The
   detail noise the march needs is sitting in the file already.

---

## 2. The weather map is already there, and that is the whole design

Guerrilla's Nubis — the class of technique 0015 named — is built on a **2D weather map**
crossed with a **vertical height profile** and eroded by **3D detail noise**. The weather
map is a low-frequency 2D texture of coverage, cloud type and precipitation over the world;
the third dimension is invented inside the march.

Loom already has that 2D map, authored in Rust, and it is the map the rain reads:

```rust
// crates/loom_field/src/lib.rs:497
Field { name: "clouds_at", body: [coverage, n, c(0.0)] }
```

An `Expr` tree over `(X, Z, T)` — `Y` never appears in it — codegen'd to Slang by
`build.rs`, pinned numerically by `field_agree`, and consumed by `loom_rain::cover_at` on
the CPU and `cloudCoverAtTime` on the GPU. ADR 0016 argued the whole shape for exactly this
property:

> **the clouds the eye sees and the rain the simulation feels are the same function, by
> construction**. That is not a discipline this project has to maintain; it is a property
> the codegen enforces.

**So the field does not change.** `clouds_at` stays 2D, stays CPU-authoritative, stays the
rain's input, and stays out of this decision entirely. The march samples it at the world
`xz` of each step instead of at one projected point, and *that substitution is where
parallax comes from*. Everything else the volume needs — thickness, an underside, a
silhouette — is invented in the shader from a height profile and frozen noise, none of
which any simulation reads.

This is the rare case where the expensive-looking feature costs nothing structurally,
because the structure was built for a different reason four ADRs ago and happens to be the
right one.

### The density function

```
density(p) = coverage(p.xz) · profile(h, type) · erosion(p)
```

- **`coverage(p.xz)`** — `clouds_at(p, t, params).x`, unchanged, sampled at the march point.
- **`h = (p.y − base) / (top − base)`**, clamped to 0…1.
- **`profile(h, type)`** — the vertical shape, §3.
- **`erosion(p)`** — ridged fBm from `loom_value_noise`, advected on the same wind, biting
  hardest where the profile is already weak so edges fray and the core stays solid. This is
  Schneider's remap-erosion and it is what separates cloud from fog.

---

## 3. Type is `cloud.w`, and it was already spare

`EnvironmentGpu::cloud` is a `float4` documented at `crates/loom_render/src/renderer.rs:315`
as *"x cloud cover 0..1, y metres across a cloud mass, z drift multiplier on the wind,
**w unused**"*. Grepping every read of it in `scene.slang` returns `.x` five times and
nothing else. The default is `[0.0, 1200.0, 2.5, 0.0]`.

**So the type channel needs no new uniform, no struct growth, no layout test change, and no
field output.** It is a float that has been sitting there since the block was written.

Type is one scalar, 0 to 1, and it drives three things at once:

| | stratus (0) | cumulus (1) |
|---|---|---|
| base altitude | low | high |
| thickness | thin | tall |
| profile shape | fat at the bottom, flat sharp top | narrow base, bulging middle, rounded top |

That is meteorologically the right coupling — a stratus deck really is lower and thinner
than a cumulus field — and it means one authored float produces two visibly different skies
without a second parameter to keep consistent with the first.

### The default is derived from cover, so no scene has to be re-authored

`cloud_type` is `Option<f32>` on the `Sky` component, reusing the override pattern
`components.rs:1163` already uses for `cloud_cover`. When it is `None`, type is computed on
the **CPU at scene load** from cover — low cover is a cumulus field, high cover is an
overcast ceiling, which is what those numbers physically mean — and the shader reads a
plain float with no sentinel and no per-pixel branch.

Nine scenes author cover today (`cascade`, `croft`, `lanternhead`, `mood_deep`,
`mountain_pass`, `puddles`, `rain_pool`, `squall`, `deeper_demo`). **None of them needs to
author type**, and each gets a sky consistent with the cover it already chose.

---

## 4. The camera stays below the base, and that is what makes the march cheap

**Human decision, 2026-09-05, recorded verbatim: "Never — always below the base".**

DEEPER is played from a boat. The camera lives at sea level and the horizon is the picture.
That constraint buys the cheap form of the march:

- Entry and exit are a closed-form slab intersection: `t0 = (base − eye.y)/dir.y`,
  `t1 = (top − eye.y)/dir.y`, for `dir.y > 0` only.
- The ray is guaranteed to start outside the volume, so there is no inside-the-volume case,
  no partial-slab bookkeeping, and no interaction with the existing height-falloff fog.
- The step count is fixed and the early-out is the soot volume's `T_MIN`.

Near the horizon `t1 − t0` runs to infinity. **That is the same degeneracy `cloudPlane` has
today** — its own comment says an unbounded coordinate into a noise lattice is *"a smeared
band of garbage rather than a distant sky"* — and `horizonFade` (`scene.slang:578`) is
already the fix for it. The march caps total length and reuses that fade unchanged.

If the eye ever does rise above the base, `skyColor` falls back to today's plane projection
in one branch. That is a graceful degrade, not a feature: see §6's reopening trigger.

---

## 5. Cost: full resolution first, and then a number

ADR 0015 deferred this for having *"no budget model"*. **The answer to no budget model is a
measurement, not three tiers of optimisation bought on speculation.**

The march is built full-resolution, inside `skyColor`, with no new pass. Then `squall` and
`mood_deep` are timed before and after, and the number decides what happens next. Both
escalations are already licensed and neither is built now:

- **Half-resolution target plus a depth-aware upsample.** A second inhabitant of the
  boundary ADR 0010 moved once already and described as *"moved, not eroded"*.
- **Temporal reprojection**, the Nubis answer proper. ADR 0073 permits it under a written
  rule — *"permitted where the accumulated state is a deterministic function of (scene,
  tick) that re-derives from a cold start"* — and a cloud march satisfies it more easily
  than the rain drops that forced the rule.

**The escape hatch is the ablation row.** `LOOM_ABLATE=cloud_volume` restores the plane
projection exactly. That gives `cargo xtask ablate` its row, gives the gate a proof the
volume is actually drawing, and gives a measured low tier for free — the same switch
serving three purposes, which is the pattern `spray_droplets` established in `aa0bd9e`.

### Load-time refusals

Per this project's rule that a constraint ships as an error with the number in it:

1. `cloud_type` outside `[0, 1]` — refused, with the authored value named.
2. `cloud_scale <= 0` — refused. It is a divisor in `clouds_at` and always was.

**A load-time refusal on camera altitude is deliberately *not* added**, and the reason is
worth stating rather than leaving as an omission: the camera moves. Checking the authored
position would pass a flythrough that climbs through the base on tick 200, which is the
case that matters. The runtime fallback in §4 covers the moving camera and the static one
with one branch, so a load-time check would be a second, weaker mechanism for a case the
first already handles.

---

## 6. What this does not settle

- **Cloud shadows on the world, and god rays.** ADR 0016's sentence stands. The march makes
  both *possible* — there is now a real transmittance along the sun ray — and neither is
  built here.
- **Rain leaving the deck.** ADR 0016 step 5 is still unbuilt: drops still spawn in a ~72 m
  box around the camera and are modulated by cover. Its own note that *"a distant curtain of
  rain crossing a bay would have to be drawn in the sky pass, and is a separate feature
  needing its own ADR"* is still owed, and the sky pass is now a volume march, which is
  where such a curtain would go.
- **Weather that evolves.** ADR 0016 drew the line — *"Anything resembling a simulated
  weather front is not [reproducible], and would be a much larger commitment"* — and this
  ADR does not cross it. Coverage still advects and does not develop. Making type and cover
  functions of a simulated front puts weather inside the determinism hash and needs its own
  ADR.
- **Flying into the deck.** Reopening trigger: **the first scene that needs a camera above
  `base` with cover above zero.** The fallback branch keeps it from being a crash; it does
  not make it look right.
- **Whether analytic detail noise is enough.** `loom_value_noise` is value noise, and value
  fBm frays wispily where Worley billows. Reopening trigger: **if the silhouette reads
  wispy rather than cauliflowered at the measured step count**, the escalation is a 128³ R8
  Worley volume — 2 MB, and the first 3D image in the codebase (`grep` for `Texture3D`
  returns nothing today; the four `Extent3D` hits are all staging-buffer copies). That is a
  new resource path and it is not bought before the picture asks for it.
- **`cloud_scale` doing two jobs.** It runs 90 m in `rain_pool` and 900 m in `croft`, and
  the small values were chosen to make *rain* vary across a 51 m scene, not to look like
  sky. Once the deck is world-space geometry that one number controls both the rain
  footprint and the cloud silhouette, which want different values. **This tension is
  pre-existing and deliberate** — ADR 0016 made them the same field on purpose, and that
  argument has not weakened. The march only makes it visible. Scenes are re-authored; the
  parameter is not split. Reopening trigger: a scene that cannot satisfy both at any value.

---

## 7. Rejected alternatives

**Particles or billboards.** Rejected by ADR 0015 on two measured grounds — clouds are
large and overlapping, which is the case Latta's fill-rate argument condemns rather than
excuses, and they are alpha rather than additive, so they need the depth sort the whole
particle path was built to avoid. **Unchanged, still rejected, not reopened by this ADR.**

**Making `clouds_at` three-dimensional.** Rejected. The rain query wants a 2D footprint —
`cover_at` takes an `[f32; 3]` and uses two of it — so a third dimension buys the
simulation nothing while putting cloud *interior* structure inside the determinism hash and
inside `field_agree`. Nubis's own architecture keeps the map 2D for the same reason. The
engine already had the right shape; changing it would be work spent to make the design worse.

**A baked 3D noise volume, up front.** Rejected *for now*, on the count that there are zero
3D images anywhere in the codebase against a frozen, generated, agreement-tested
`loom_value_noise` that costs nothing to call. §6 carries the reopening trigger and the
size.

**Half-resolution and temporal reprojection, up front.** Rejected as unmeasured
optimisation. ADR 0015 deferred volumetrics for having no budget model; buying three tiers
of mitigation for a cost still nobody has measured repeats that mistake with more code.
§5 makes the measurement the gate.

---

## 8. Human approval

Not required by CLAUDE.md's locked table — no locked decision moves. Required by this
project's rule that a builder never promotes its own ADR.

Recorded verbatim so far, from 2026-09-05:

- On the sky to build: *"Both, via a type channel"*.
- On the camera: *"Never — always below the base"*.
- On the ask that started it: *"clouds that look realistic and good even… so they're not
  just like a 3D or 2D backdrop or something like that. Like, they're real clouds that have
  real rain that come out of them."*

**The last clause is not delivered by this ADR** and should not be read as approved by it.
Rain leaving the deck is §6's second bullet, it is ADR 0016's unbuilt step 5, and it is a
separate decision with its own record.
