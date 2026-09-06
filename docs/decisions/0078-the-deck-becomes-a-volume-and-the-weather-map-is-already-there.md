# ADR 0078 — The deck becomes a volume, and the weather map was already there

- **Date:** 2026-09-05
- **Status:** **accepted** (2026-09-05, human — "promote the four ADRs to accepted"). Built and green on all six
  checks at the time of acceptance.
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

- ~~**Cloud shadows on the world, and god rays.**~~ **Both built, and both are why this
  section should be read with its dates.** The march made them possible by giving the sun ray a
  real transmittance, and each became its own ADR once the trigger fired: god rays are
  **ADR 0080**, cloud shadows on the world are **ADR 0081**. ADR 0016's sentence no longer
  stands.
- ~~**Rain leaving the deck.**~~ **Built as ADR 0079**, which refuses ADR 0016 step 5 as
  literally written and builds the shower as a medium instead. Drops still spawn in their own
  ~64 m box and are modulated by cover — deliberately, and 0079 §3 says why. Its own note that *"a distant curtain of
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
  argument has not weakened. The march only makes it visible.
  ~~Scenes are re-authored; the parameter is not split.~~ **FALSE WHEN WRITTEN — see
  Addendum 1.** Reopening trigger: a scene that cannot satisfy both at any value.

---

## Addendum 1 — `cloud_scale` is not a C4 authoring problem, it is the slab's thickness

**Date:** 2026-09-05, on building C2.

§6 above filed `cloud_scale`'s double duty as a look problem to be settled later by
re-authoring the nine scenes. Three renders say that was wrong in both halves.

The deck was built with fixed altitudes — base 600–1000 m, top 1000–2600 m — giving a slab
up to 1600 m thick. `squall` authors masses **260 m** across. So every cloud was six times
taller than it was wide, every view ray crossed several of them, and the integral averaged
the weather map into grey mush hung with vertical streaks off the noise column. The plane
projection never met this, because it had no thickness to disagree with.

**A cloud is roughly as tall as it is wide**, so the vertical extent follows the authored
horizontal one: `thickness = cloud_scale · lerp(0.35, 0.90, type)`, clamped to
`[120, 1200]` m. Extinction follows it too, expressed as optical depth *through the slab*
(`CLOUD_DEPTH_SOLID = 4.0`) rather than per metre — a fixed per-metre figure would make a
90 m deck a haze and a 1200 m one a wall.

**And re-authoring could not have fixed it.** `rain_pool`'s 90 m is chosen to make rain vary
across a small scene; raising it to look like sky would break the thing the number is for.
The parameter still is not split — it now means the same thing in both jobs, which is what
ADR 0016 always intended.

## Addendum 2 — coverage saturates, and the march met that trap a second time

**Date:** 2026-09-05, on building C2.

The march first masked density with the coverage channel alone. `squall` at cover 0.45
looked right; `lanternhead` at 0.85 and `mood_deep` at 1.0 came back as featureless wash.

That is the signature ADR 0015 already recorded, in its own words: *"Coverage saturates, so
under an overcast sky both taps read 1.0, every difference is zero and the deck shades to
one flat tone. That is not a hypothetical: it is what the first version of this did."* It
wrote that about the sunward shading tap. It is equally true of a volume, and this ADR did
not carry the warning forward — so the same trap was walked into in a new place.

`clouds_at` returns density beside coverage for exactly this reason. The march's horizontal
mask is now `coverage · lerp(0.30, 1.0, density)`, so coverage gates whole regions to zero
at low cover and the raw fBm carries the interior at high cover. **A deck has structure at
every cover.**

`cloudCoverAtTime` cannot be reused for it: at `cover >= 1.0` it short-circuits to a flat
1.0 without evaluating the field. That is right for the rain and discards precisely what a
volume needs, so the march has its own accessor.

## Addendum 3 — the cost is dominated by reflections, not by the sky

**Date:** 2026-09-05. The measurement §5 asked for, at 960x640, `--sim 300`:

| | forward | water | graph |
|---|---|---|---|
| `squall` plane | 0.040 ms | — | 2.21 ms |
| `squall` volume | 0.438 ms | — | 3.61 ms |
| `mood_deep` plane | 0.068 ms | 0.191 ms | 0.398 ms |
| `mood_deep` volume | 0.368 ms | 1.66 ms | 1.81 ms |

**The water pass is 83% of `mood_deep`'s added cost**, and that was not predicted anywhere
in this ADR. Water reflections call `skyColor`, so the march runs once per water pixel as
well as once per background pixel. Scaled to 1920x1080 the deck costs on the order of
**+4.7 ms**, which is over a quarter of a 16.7 ms frame.

**This changes which escalation §5 should reach for.** A half-resolution screen-space pass —
the first option §5 names — would optimise the *smaller* half and do nothing at all for a
reflection ray, which is not indexed by screen position. The escalation that matches the
measurement is a direction-indexed low-resolution cloud map that both the background and
every reflected ray sample. §5's ordering stands as written for the sky; it was silent on
reflections because this ADR did not know they were the cost.

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

**Accepted 2026-09-05.** The human's words, verbatim: *"promote the four ADRs to accepted"* —
0078, 0079, 0080 and 0081 together, after the whole series was green on all six checks and
after the measurement corrections each of them carries were made. Recorded verbatim per this
project's rule, so the scope of what was accepted is not relitigable later.

## Addendum 4 — the escalation is a direction-indexed map, and it is 3x

**Date:** 2026-09-05. Built on the human's instruction to escalate.

Addendum 3 measured the cost and said §5's ordering was wrong for it. This is what
was built instead: the deck is marched **once per frame** into a 1024x512 equirectangular
`R16G16B16A16_SFLOAT` image, and the background and every reflected ray sample it.

**Measured at 1920x1080, `--sim 300`, 20 frames, before and after, from two builds:**

| | forward | water | cloud_map | graph |
|---|---|---|---|---|
| `mood_deep` inline march | 0.981 | 3.428 | — | **4.801 ms** |
| `mood_deep` map | 0.132 | 0.298 | 0.754 | **1.576 ms** |
| `squall` inline march | 3.793 | 5.135 | — | **9.435 ms** |
| `squall` map | 0.074 | 2.439 | 0.710 | **3.724 ms** |

**3.0x and 2.5x**, and the shape matters more than the ratio: the march is now a
*fixed* 0.71-0.75 ms that does not scale with output resolution or with how much
water is on screen. `squall`'s residual 2.4 ms of water pass is the FFT ocean, not
the sky.

**1024x512 is derived, not picked.** `squall`'s 260 m masses subtend ~5 degrees at
3 km; the map gives 0.352 deg/texel, so 14 texels across the tightest mass this
repository authors. 2048x1024 would be 2.10 MP of marching against a 1080p frame's
2.07 MP — no saving at all. Upper hemisphere only, because §4's constraint says the
deck is never below the eye.

**The predicted quality cost did not materialise at the size it was predicted at.**
The estimate here was 7.5x softer than a per-pixel march, from texel-versus-pixel
angular size. That arithmetic is right and the conclusion was wrong: clouds are
low-frequency, and at 14 texels per mass with bilinear filtering the map renders are
not visually distinguishable from the inline march on `squall` or `cascade`.
`lanternhead` is unchanged — still the C2 regression, still C3's to fix.

**One thing this ADR reasoned wrongly about Vulkan, caught by the layers.** The map
pass was written binding no descriptor set, on the reasoning that
`cloudMapFragmentMain` samples nothing. `VUID-vkCmdDraw-None-08600` refused it: Slang
compiles `scene.slang` as one module and set 3 counts as statically used. Set 3 is now
bound and never accessed, which is the same shape `renderer.rs` already documents for
the water draw. Recorded because CLAUDE.md's never-do #5 exists for exactly this and
the reasoning looked sound right up until it was run.

## Addendum 5 — what it finally costs, and the two authoring tasks that were not needed

**Date:** 2026-09-05, on finishing C4.

### The cost, min of 3 reps, 24 frames, 1920x1080, `--sim 300`

| scene | projection | volume | the deck |
|---|---|---|---|
| `mood_deep` | 0.879 ms | 3.833 ms | **+2.95 ms** |
| `squall` | 3.017 ms | 4.837 ms | **+1.82 ms** |
| `lanternhead` | 4.529 ms | 6.342 ms | **+1.81 ms** |

**Min of three, and that matters here.** Single-shot readings of the same binary spread up
to 55% — `squall` measured 4.837 and 7.486 minutes apart. Any figure in this ADR taken as one
sample should be read as indicative; these three are not.

So the finished deck is **+1.8 to +3.0 ms**, 11-18% of a 16.7 ms frame, for parallax, a
silhouette, an underside, self-shadowing by a real light march, and forward scattering.

### §5's remaining escalations are not triggered

The direction-indexed map (Addendum 4) was the escalation and it is built. Neither of §5's
others — half-resolution with a depth-aware upsample, or temporal reprojection under ADR 0073
— is warranted at this cost, and neither should be built without a new measurement saying so.
**The reopening trigger is a frame that cannot afford 3 ms**, which on this hardware at this
resolution is not the case.

### A clear sky is byte-identical, and that is checked rather than assumed

`loom compare --channel 0 --fraction 0 --worst 0` reports **0 differing pixels** between the
volume and `LOOM_ABLATE=cloud_volume` on `deeper_demo` at its authored rung (a clear sky) and
on `materials` (which never mentions clouds). The feature costs nothing and changes nothing
where there is no cloud — the `cover <= 0` short-circuit holds all the way through.

### Both of C4's authoring tasks turned out to be unnecessary

**Re-authoring `cloud_scale` is not needed, and §6 predicted wrongly that it would be.** All
nine scenes authoring cover were rendered and looked at. None shows the grain §6 feared:
`rain_pool` at 90 m and `lanternhead` and `squall` at 260 m all read as cloud rather than as
texture, because Addendum 1 made thickness follow `cloud_scale` and the masses are therefore
roughly isotropic at every authored scale. **Not one scene file is edited by C4.**

**No scene needs an explicit `cloud_type` either.** The cover-derived default is right
everywhere it was checked: `cascade` 0.35, `croft` 0.38, `mountain_pass` 0.45 and `squall`
0.45 all come out cumulus and read as broken cloud; `lanternhead` and `rain_pool` at 0.85 and
`puddles` at 1.00 come out stratus and read as ceilings. The plan's rule was *"a scene that
does not need it does not get it"*, and none does.

**One honest limit, recorded rather than fixed.** `rain_pool` (90 m masses) and `puddles`
(solid cover) render as smooth overcast with little visible structure. That is partly correct
— a solid ceiling has little structure from below — and partly the map's 0.352 deg/texel
filtering out masses that subtend under a degree at distance. It is the one place Addendum 4's
resolution choice is visible. Neither scene is about its sky, so it is not worth a re-author;
if a scene ever wants fine cloud texture *and* small masses, that is the trigger to revisit the
map's resolution.
