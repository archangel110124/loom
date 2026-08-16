# The post-processing stack — plan and ADR

**Status: designed, not built.** Produced by a twelve-agent review against the
tree at `b4e25e4`. Part 0 is the most valuable section: five corrections, each
checked against source, `vulkaninfo` on this machine, or arithmetic shown in
place. It killed both designs' choice of tonemap operator.

## Why this exists, in one table

Every one of these was measured in a single session's work, and all four are
the same cause — an 8-bit fixed-point target with no exposure control:

| symptom | what it actually was |
| --- | --- |
| sprite fire clipped to white | red pinned after ~2.6 additive layers |
| a campfire lit a white disc | `I*albedo*h/d^3` clamped; the "glow" was channels leaving the clamp one at a time |
| water washed to sky at grazing angles | no headroom |
| the fire ramp is capped below 1.0 | so the scene's own light could not rotate it toward white |

The sharpest evidence is that **a campfire has to be authored at intensity
1.35**, because the ceiling before anything clips is about 1.5. The renderer's
dynamic range has been choosing the art.

---

**Read-only. I created, edited, deleted and `git`-ed nothing in the repo.** The only file written anywhere was `/tmp/vi.txt` (a `vulkaninfo` dump). Every number below is from the tree at `b4e25e4`, from `vulkaninfo` on this box, or from `python3 -c` arithmetic I show.

---

# Part 0 — Corrections that change the plan

## 0.1 Both plans chose a curve that crushes every dark scene in the library, and neither ran it

Both pick **Khronos PBR Neutral**. Plan 2 says it "is roughly identity below its 0.76 compression knee" and predicts `cave` "moves least". The `peak < startCompression` early-out skips the *compression*; the black-point subtraction above it runs unconditionally, and below linear 0.08 it is `x − 6.25x²`, which is proportional, not fixed. Measured:

```
linear   code today   PBR Neutral   shoulder (§0.2)
 0.002        7            0             7
 0.008       22            1            22     <- campfire.loom's authored night zenith
 0.020       39            8            39
 0.055       66           37            66     <- GROUND_COLOR
 0.100       89           69            89
 0.500      188          181           188
 0.720      221          215           221
```

`campfire.loom`'s sky goes **code 22 → code 1**. `cave` — authored dark, heavy fog, one of the three 0.000 shimmer controls — moves *hardest*, not least. Plan 2's own re-bless prediction is exactly inverted, and its own stated rule ("a scene moving out of tier is a finding") would flag it.

Worse, on the thing the ADR exists for. Same rung `fireRamp` uses, `(0.62, 0.210, 0.020)` (`assets/shaders/scene.slang:1987`), HSV of the sRGB-encoded output — `saturation / hue°`:

```
       clip (today)   PBR Neutral    shoulder
 x1     0.811/31.3    0.961/34.6    0.811/31.3
 x4     0.686/53.5    0.524/25.5    0.802/31.2
 x16    0.400/60.0    0.213/21.1    0.803/31.5
 x64    0.000/ 0.0    0.067/21.2    0.804/31.3
```

**PBR Neutral desaturates the fire faster than clipping does** — 0.213 against 0.400 at ×16. That is the identical failure, on the identical metric, that Plan 1 used to reject the full ACES fit ("rejected on measurement, not taste"), caused by the same mechanism: a `desaturation = 0.15` lerp toward white on any peak past the knee. Plan 1 pasted `START_COMPRESSION` and `DESATURATION` into its own ADR without reading eight lines further down, and its comparison table **has no row for the operator it selects**.

## 0.2 The correction: delete both halves. Six lines, and it makes the re-bless predictable

```slang
float3 shoulder(float3 c) {
    float peak = max(c.r, max(c.g, c.b));
    if (peak < KNEE) { return c; }          // KNEE = 0.76
    float d = 1.0 - KNEE;
    return c * ((1.0 - d * d / (peak + d - KNEE)) / peak);
}
```

Hue constant to **0.3° across six stops**, saturation constant to 0.01 — literally the property `scene.slang:1977-1990` caps the ramp to protect and neither candidate delivers. And it is **exact identity below linear 0.76**, which buys the thing critique 3 asked for and neither plan has: *which references move is computable from the current PNGs before a line of Vulkan is written.*

I ran that census over all 25 references (pure-Python PNG decode, no writes):

```
scene            px>226    pct     clipped@255       scene            px>226    pct    clipped@255
cave                  0   0.00%           0         beach              1711   2.67%          44
grass_slope           0   0.00%           0         explosion          1433   2.24%         803
meadow                0   0.00%           0         forest             1585   2.48%           0
primitives            0   0.00%           0         homestead          1150   1.80%         852
smoke                 0   0.00%           0         ocean              3342   5.22%         236
windy                 0   0.00%           0         proving_ground    22749  35.55%       18345
materials             3   0.00%           0         puddles            4484   7.01%         120
campfire            131   0.20%          34         shore              5252   8.21%         115
ground              147   0.23%           0         squall             5628   8.79%         167
rain_impact          37   0.06%           0         splash             1202   1.88%          28
rain_overhang        54   0.08%           0         water_crate        1201   1.88%          34
river                17   0.03%           0         rain_gantry         124   0.19%          87
underwater           20   0.03%           0
```

**Six references must be bit-identical; `materials` (3 px, against a 64-px `fraction` gate) must pass unchanged.** That is the acceptance test, it is falsifiable, and it exists before the code does. Under PBR Neutral all 25 move and nothing is predictable.

Where the critics disagree — critique 2 deletes the offset *and* the desaturation, critique 3 deletes only the offset — **I took the conservative reading and deleted both**, because keeping the desaturation reintroduces the measured hue-rotation-to-white that this project's most carefully-reasoned shader constant exists to prevent.

## 0.3 Fourteen pipelines carry a format argument, not one. Both plans miss most of them

Plan 1: "`Msaa::new(...)`. The `format` parameter is deleted. **This is the single most valuable line of the slice**: it is the one place the two render paths were structurally permitted to differ."

It is not the one place. `crates/loom_render/src/viewer.rs:296-299` says so itself:

> "The pipeline is built for the *swapchain's* format… **Dynamic rendering bakes the attachment format into the pipeline, so this cannot be shared.**"

The viewer builds **seven** scene pipelines against that format — `viewer.rs:306` (mesh), `:317` (sky), `:320` (particles), `:327` (grass), `:340` (water), `:355` (rain), `:363` (splashes). The offscreen path builds the same seven against `COLOR_FORMAT` — `renderer.rs:1021, :1029, :1035, :1042, :1051, :1065, :1077`. All fourteen rasterise into `loom.msaa_color`, so **all fourteen must become `HDR_FORMAT`** or `VkPipelineRenderingCreateInfo::pColorAttachmentFormats` disagrees with the attachment and every draw of that pipeline is a validation error. Plan 1's change list does not include `renderer.rs:1018-1077` at all and never mentions viewer pipelines. Plan 2 enumerates the offscreen seven and misses the viewer's seven.

## 0.4 `A2B10G10R10` is not merely useless, it is a regression — and the format is verified here, not recalled

`vulkaninfo --show-formats`, `deviceName = NVIDIA GeForce RTX 4090`, `FORMAT_R16G16B16A16_SFLOAT` → `optimalTilingFeatures` carries `COLOR_ATTACHMENT_BIT`, **`COLOR_ATTACHMENT_BLEND_BIT`**, `SAMPLED_IMAGE_BIT`, `STORAGE_IMAGE_BIT`, `SAMPLED_IMAGE_FILTER_LINEAR_BIT`, `TRANSFER_SRC/DST`. `framebufferColorSampleCounts = {1,2,4,8}`. Blending and 4× MSAA both available; the format is not the constraint.

There is no `A2B10G10R10_SRGB`, so storing linear in it puts `campfire`'s 0.008 zenith on a **12.2% quantisation step against the 6.68% the 8-bit sRGB target already achieves there**. Both plans reject it; only one gives that reason, and it is the decisive one.

## 0.5 fp16 turns a harmless authoring mistake into a NaN pixel — neither plan has the fix in the right place

`pointLights` (`scene.slang:280-300`) floors the denominator at `max(d2, 1e-4)` — 1 cm — and **never clamps the result**:

```
d = 0.020 m, intensity 30  ->  75,000    (fp16 max is 65,504: +inf)
d = 0.010 m, intensity 30  -> 300,000    (+inf)
```

Any intensity above ~6.6 overflows where geometry touches a light centre. Today it clamps harmlessly to 1.0 on store; after the format change it writes `+inf` into a multisampled attachment, `AVERAGE` of `(inf,x,x,x)` is inf, fog's `lerp` gives `inf*0 = NaN`, and `imagediff.rs:31-34` names a NaN pixel as exactly what `worst: 72` exists to catch. Plan 1's `FIREFLY_CLAMP = 16384` sits **in the tonemap, after the resolve**, so by its own stated reasoning it is in the wrong place, and `clamp(NaN,·,·)` does not help. Plan 2 has no clamp. **One `min()` in `pointLights`, where the singularity is.**

## 0.6 `bad_intensity.loom` cannot be rendered

Plan 1 slice 3: "give it the clipped-pixel assertion". Its own header (`assets/test/bad_intensity.loom:1-10`): *"M1 error-path fixture. Parses as TOML; **must FAIL validation**… `field_out_of_range`, `Light.intensity`, `value = 40000.0`."*

And no new fixture is needed either — `proving_ground` already has **18,345 pure-white pixels (35.6% of the frame above code 226)** and is in `GOLDEN`. The gate already contains the subject. Rung 2: it's already in the codebase.

## 0.7 Plan 2's S1 thesis is false, so the five-slice spine buys nothing

S1 ships `saturate()` at the end of the graph and claims "the picture is unchanged, checked by `cargo xtask image` passing with no re-bless". Its own audit gives the counterexample: samples `[40.0, 0.1, 0.1, 0.1]` resolve to **0.325** today and **10.075** after, and `saturate` makes that 1.0 — code 156 → 255, a delta of 99 against `worst: 72`. Plus four of the five sRGB round trips disappear and additive sums stop clamping per blend step. **S1 moves every clipping scene and blesses 19 references to buy an attributability story that §0.2's prediction gives for free.** Rejected; three slices, two blesses.

## 0.8 Smaller, checked

| claim | reality |
|---|---|
| Plan 1: strip `TRANSFER_SRC` from `loom.color_target` | Contradicts the rule it quotes three paragraphs earlier (`renderer.rs:832-838`: "a usage flag changes no pixel"). Keep it — it is the only way to dump the pre-tonemap frame when the curve looks wrong. |
| Plan 1: "an fp16 target without a tonemap readbacks raw half-floats… not a green intermediate state" | False. The readback follows the last pass that wrote a pixel (`renderer.rs:1706-1713`). Right conclusion, wrong reason. |
| Plan 2: ±1 LSB dither moves CMAA2's `sqrt(luma)` by 0.0004 | It is **0.00393**. Conclusion survives; a wrong number inside an ADR does not. Dither is dropped anyway (§ "not doing"). |
| Critique 1: raise `TIMED_PASSES` | `renderer.rs:784` is 12; offscreen has 6 passes, viewer 7 (`grep graph.pass`). Tonemap makes it 7 and 8. **No change needed** — the concern was real only for the bloom we are not building. Its doc comment ("declares two passes") is stale; fix in passing. |
| Both: rain draws "after the resolve, single-sampled" | Stale, in the brief and in `renderer.rs:3568`'s comment. `renderer.rs:1056-1069` builds it at the scene's sample count and `:2076` declares `(ms_color, ColorWrite)`. **Only the UI is after the resolve.** |

---

# Verdict

Ship **one full-screen pass, one image, one six-line curve, one `f32` on `Environment`, one `min()` in `pointLights`** — Plan 1's shape with Plan 2's `Post`-owns-its-image structure, and the operator both plans chose replaced by a pure shoulder that is exact identity below linear 0.76. That identity is the whole engineering argument: it turns "re-bless 25 opaque hashes" into a prediction — **six references must not move at all** — computable before any code exists, and it is the only version of this change whose gate can tell a correct curve from a stubbed one.

Bloom, dither, SSAO, grain, CA, vignette, DOF, LUTs and auto-exposure are refused *in the ADR*, with reasons, so a third boundary move argues against a written decision rather than filling an unstated gap.

---

# ADR 0019 — The frame is computed in float and collapsed once

`docs/decisions/` runs to `0017-raindrops-become-stateful.md`; **0018 is reserved** for the water-refraction split by `docs/design/WATER-REFRACTION-PLAN.md:405`. This is **0019**, filename `0019-hdr-rendering-and-one-tonemap-pass.md`.

````markdown
# ADR 0019 — The frame is computed in float and collapsed once

- **Date:** 2026-08-15
- **Status:** **proposed** — human approval required before any code lands.
- **Decision touched:** the build brief's "no post-process stack before Phase 8".
  ADR 0010 moved that boundary once and said so in as many words. **This is the
  second move, and it re-draws the boundary behind itself** by naming what stays
  outside — because a boundary moved twice without being re-drawn is precisely
  the erosion ADR 0010 was careful to say it was not doing.

## What ADR 0010 permits and forbids, quoted

`0010-non-temporal-aa-is-insufficient.md:4-6`:

> **accepted** (2026-08-12, human). A CMAA2-class full-screen pass is authorised.
> The build brief's "no post-process stack before Phase 8" boundary is hereby
> **moved, not eroded** — this pass is its first and, until Phase 8, only
> inhabitant.

and `:70-75`:

> Add **one** non-temporal full-screen AA pass, CMAA2 or SMAA 1x class, run after
> the forward pass and before readback.
>
> This is a real scope change. It introduces the first post-process pass in the
> renderer, and the build brief defers the post stack to Phase 8. It is not a
> slippery slope by itself, but it is the first step onto one, which is why it is
> here rather than in a commit.

**Permits:** exactly one non-temporal, single-frame, full-screen anti-aliasing
pass of CMAA2/SMAA-1x class, owned by the render graph (`0010:95`), with the
golden set re-blessed once in the commit that lands it (`0010:96`).

**Forbids:** every other inhabitant of the post stack before Phase 8 — the word
is "only" — and, separately and for a different reason (`0010:79-82`), any pass
that makes a frame a function of previous frames.

A tonemap is not anti-aliasing. It is a second inhabitant, and ADR 0010 named
this document's price: an ADR, a stated risk, a pre-declared measurement, and a
deliberate re-blessing.

**One correction to ADR 0010 in passing:** its `:112` and `:146` say CMAA2 is
"behind `LOOM_CMAA2=1` — off by default". `cmaa2.rs:107-110` reads
`std::env::var_os("LOOM_CMAA2").is_none_or(|v| v != "0")` — **on** by default,
and all 25 references were blessed with it on.

## Context — the renderer's dynamic range has been choosing the art

`COLOR_FORMAT` is `R8G8B8A8_SRGB` (`crates/loom_render/src/renderer.rs:43`).
Fixed point. Every blend clamps at 1.0 with no rolloff, **per sample, before the
MSAA resolve**, and there is no tonemapping anywhere — `grep` settles it. That is
11.69 stops from code 1 (linear 3.035e-4) to 1.0, and **zero stops above diffuse
white**.

Four systems have already been shaped by that number. The first two changed a
*technique*, not a constant:

1. **Fire stopped being sprites.** `assets/shaders/scene.slang:1875-1885`: "The
   colour target is `R8G8B8A8_SRGB` — fixed point, so every additive blend clamps
   at 1.0 with no rolloff anywhere. Measured on the sprite path, red pinned after
   2.6 overlapping particles against about thirty alive… A single quad has
   overdraw of exactly one. Nothing can clip."
2. **A campfire cannot be authored with a physical intensity.**
   `assets/test/campfire.loom:123-131` authors **1.35** and writes out the
   arithmetic: at 22, red reached 1.0 by 1.77 m, green by 1.35 m, blue by 0.89 m,
   and "the 'soft glow' around it was the channels leaving the clamp one at a
   time rather than any falloff… the ceiling before anything clips at all is
   about 1.5." The schema says "Interior lights are typically 100-800"
   (`crates/loom_scene/src/components.rs:84`), default 100.0.
   `blockout.loom:91`, `office.loom:50`, `workshop.loom:65`,
   `proving_ground.loom:178` author 800/800/700/260 into the same unscaled path
   (`crates/loom_cli/src/main.rs:2328`). **A 600× spread across one component.**
3. **`fireRamp` is capped below 1.0 in every channel**, peak
   `(0.72, 0.640, 0.480)` — `scene.slang:1977-1990`: "the moment anything else
   adds light — the scene's own `Light`, a second flame behind — red is already
   at the clip and the sum rotates orange toward yellow toward white."
4. Constants chosen against the clamp: the sun disc at 0.9 (`:463-466`),
   `CLOUD_LIT` 0.78 (`:489-493`), `WATER_FOAM_ALBEDO` (`:3013`),
   `RAIN_STREAK_BRIGHTNESS` 0.42 (`rain.slang:168`).

**Three things it would be dishonest to claim.** `WATER_CLARITY_ROUGH = 0.22`
(`scene.slang:2950`) is a roughness-aware Fresnel — correct physics that also
cured a washout, and it stays. `RAIN_MIN_PIXELS = 2.5` is a sub-pixel coverage
fix on a salt metric. `WATER_FOAM_FINE_RANGE` is an aliasing bound. None is a
dynamic-range workaround, and fire-as-a-level-set stays right under HDR because
overdraw of exactly one is a good property regardless — only its *stated reason*
dissolves.

## The decision

**1. The scene's colour images become `R16G16B16A16_SFLOAT`** —
`loom.msaa_color`, `loom.scene_opaque`, `loom.color_target`, and the viewer's
scene image. Depth is unchanged. 30.0 stops, **16 of them above diffuse white**,
at a constant 0.098% relative step against sRGB8's 1.0–6.8% across the band these
scenes actually paint in. Verified on this machine rather than recalled:
`vulkaninfo --show-formats`, `deviceName = NVIDIA GeForce RTX 4090`,
`optimalTilingFeatures` for that format carries `COLOR_ATTACHMENT_BIT`,
`COLOR_ATTACHMENT_BLEND_BIT`, `SAMPLED_IMAGE_BIT`, `SAMPLED_IMAGE_FILTER_LINEAR_BIT`
and `STORAGE_IMAGE_BIT`; `framebufferColorSampleCounts = {1,2,4,8}`.

**2. One full-screen `tonemap` pass** between the resolve and `cmaa2_edges`,
writing a new `R8G8B8A8_SRGB` image (`B8G8R8A8_SRGB` in the window). The hardware
still does the sRGB encode on write, so `renderer.rs:36-43`'s sentence — "the
bytes read back for the PNG are already in the space the PNG says they are in" —
remains true word for word. What moves is *where*: from the first fragment write
to the last, from five round trips through 8-bit sRGB per frame to one.

**3. `Environment.exposure: f32`, default 1.0**, a linear multiplier applied as
`tonemap(hdr * exposure)`. Fixed, authored, diffable, beside `sun_strength` and
`ambient`. Never adaptive.

**4. `pointLights` clamps its sum.** `max(d2, 1e-4)` is a 1 cm floor with no
ceiling on the result; at intensity 30 and d = 2 cm the term is 75,000, and fp16
saturates at 65,504. An `inf` survives the resolve and becomes a `NaN` pixel that
no tolerance forgives — `imagediff.rs:31-34` names exactly that case. One `min()`.

### The curve: a pure shoulder, and the alternatives were measured

```
float3 shoulder(float3 c) {
    float peak = max(c.r, max(c.g, c.b));
    if (peak < KNEE) { return c; }          // KNEE = 0.76
    float d = 1.0 - KNEE;
    return c * ((1.0 - d * d / (peak + d - KNEE)) / peak);
}
```

Measured on `fireRamp`'s deep-amber rung `(0.62, 0.210, 0.020)`
(`scene.slang:1987`), HSV of the sRGB-encoded output — saturation / hue°:

```
       clip (today)   ACES full    AgX      PBR Neutral    shoulder
 x1     0.811/31.3   0.721/37.8  0.498/32.8  0.961/34.6   0.811/31.3
 x4     0.686/53.5   0.332/45.1  0.300/36.5  0.524/25.5   0.802/31.2
 x16    0.400/60.0   0.081/49.4  0.130/40.1  0.213/21.1   0.803/31.5
 x64    0.000/ 0.0   0.014/54.2  0.031/43.0  0.067/21.2   0.804/31.3
```

- **The full ACES fit is rejected on measurement**: 0.081 at ×16 against
  clipping's own 0.400. Its RRT bleaches highlights toward white, removing
  exactly what the art protects.
- **Khronos PBR Neutral is rejected on the same measurement**: 0.213 at ×16, also
  worse than doing nothing, for the same reason — a `desaturation = 0.15` lerp
  toward white past the knee. **And its unconditional black-point subtraction
  (`x − 6.25x²` below 0.08) takes `campfire.loom`'s authored 0.008 sky from sRGB
  code 22 to code 1**, and `GROUND_COLOR` at 0.055 from 66 to 37. It crushes
  every dark scene in the library before it improves a single highlight.
- **AgX preserves hue best of the published operators and desaturates everything,
  everywhere, permanently** — 0.498 at ×1 against 0.81. That is a *look*, and
  looks are the hardest thing to reverse.
- **The shoulder holds hue to 0.3° and saturation to 0.01 across six stops**,
  which is literally the property `fireRamp`'s cap exists to protect, and it is
  **exact identity below linear 0.76**.

That identity is the engineering argument, not an aesthetic one. It makes the
re-blessing predictable: **six references (`cave`, `grass_slope`, `meadow`,
`primitives`, `smoke`, `windy`) contain no pixel above sRGB code 226 and must be
bit-identical; `materials` has three such pixels, under the 64-pixel `fraction`
gate, and must pass unchanged.** That set was computed from the existing PNGs
before any code was written, and it is the acceptance test: **if a scene with
nothing above diffuse white moves, the curve is wrong.**

It also means the flame's white core is produced by `fireRamp`'s own top rung
rather than by a tonemapper bleaching it — the art decides, which is the correct
division of labour and the one this project has been enforcing by hand.

### Ordering: tonemap before CMAA2, and CMAA2 does not change

- **Everything that blends is upstream.** Blending is in the ROP, after the
  fragment shader, so a per-fragment tonemap gives the framebuffer `Σ T(cᵢ)`, not
  `T(Σ cᵢ)`, and `Σ T(cᵢ)` is unbounded in n.
- **Rain is upstream.** Rain is no longer an overlay: it rasterises at the
  scene's sample count into `loom.msaa_color` (`renderer.rs:2074-2077`) and
  *performs the colour resolve* when MSAA and rain are both on (`rain_resolves`,
  `renderer.rs:1737`). Only the UI is after the resolve.
- **CMAA2 is downstream and its shaders are not touched.** Its detector computes
  `dot(sqrt(max(linearRgb, 0.0)), rec601)` (`cmaa2_edges.slang:104`) against
  `EDGE_THRESHOLD = 0.07` (`:25`); `sqrt` stands in for the sRGB encode and is
  only that on [0,1]. `sqrt(40) = 6.3` clears a 0.07 absolute threshold by two
  orders of magnitude and the pass becomes the blur it was engineered not to be.
  Intel's own omission list (`cmaa2.slang:59`) includes the HDR range handling.
  Independently: averaging does not commute with a nonlinear curve, so
  AA-before-tonemap turns a 100.0/0.5 edge into 50.25 → 0.98 beside a 0.33
  neighbour — the step moved one pixel rather than softening.
- **The editor UI stays between them** (viewer only). `ui.rs:82` sets
  `srgb_framebuffer: false`; egui hands over final display colours and a tone
  curve washes the panels. It already draws single-sampled after rain
  (`viewer.rs:1522-1551`); it now targets the tonemap's output. It stays
  CMAA2-filtered, which `viewer.rs:1132-1136` records as deliberate. **The
  offscreen path has no UI pass, and that must remain the only structural
  difference between the two graphs.**

## Alternatives rejected

- **A tonemap in the forward fragment shader, no format change.** The honestly
  cheaper option, and it fixes two of the four reported symptoms — the campfire's
  disc and water's grazing-angle wash are *opaque* draws with
  `blend_enable(false)` (`renderer.rs:3302-3303`, `:3437-3438`) whose sums
  complete inside one invocation of `fragmentMain` (`scene.slang:280-300`, called
  at `:1709`). It cannot fix the two that are about fire, and the arithmetic is
  not a slogan: with fragments at linear 0.5, no tonemap clips at n=2,
  per-fragment Reinhard at n=4, tonemapping the sum never. **One stop, against
  the ~30 alive particles `scene.slang:1877` measured.** It also applies at four
  entry points instead of one; it double-compresses the image water refracts
  (`renderer.rs:1961` declares `(opaque_color_id, ShaderRead)`,
  `scene.slang:3145` fetches it); and it leaves the per-sample clamp ahead of the
  MSAA resolve.
- **`A2B10G10R10_UNORM_PACK32`.** Zero headroom, so it fixes nothing — and there
  is no sRGB variant, so storing linear puts `campfire`'s 0.008 zenith on a
  **12.22% quantisation step against the 6.68% the format it replaces already
  achieves there**. It would make the night sky worse. An HDR10 *presentation*
  format, not a render target.
- **`B10G11R11_UFLOAT_PACK32`.** Same 16 stops of headroom at half the bandwidth,
  and genuinely tempting. Rejected because blue carries a 5-bit mantissa (3.125%
  step) against red and green's 1.563%, so banding in a dark blue-dominant sky
  shifts *hue* rather than level — the same artifact class as the campfire's
  three-radius ring; because it has no alpha and clamps negatives, foreclosing
  the alpha-to-coverage P4's sub-pixel streak floor wants back; and because the
  saving is 31.6 MiB at 1080p, 0.13% of a 24 GB card.
- **Auto-exposure.** The usual objection is that it is temporal, and it is
  dodgeable — ADR 0017 gave the graph buffer barriers, so a single-frame
  histogram would stay deterministic. The binding objection is different: it
  makes exposure a function of what happens to be on screen. The agent adds one
  object and every reference for that scene moves for a reason no diff explains;
  `--sim 400` and `--sim 401` disagree; and `cargo xtask shimmer` would measure
  exposure hunting rather than geometry, **destroying the three 0.000 controls**.
- **Splitting the format change from the curve into two commits.** Considered and
  rejected: a `saturate()`-only intermediate still moves every clipping reference
  (samples `[40.0, 0.1, 0.1, 0.1]` resolve to 0.325 today and 10.075 after, and
  `saturate` makes that 1.0 — code 156 → 255 against `worst: 72`), so it costs a
  second 19-reference bless to buy attributability that the identity-below-0.76
  prediction supplies for free.
- **Do nothing until Phase 8.** This is what has been happening, and it has cost
  an authoring technique, a physical unit and a colour ramp.

## Consequences if accepted

- **One pass, one image per path, no new `Access` variant** — `ShaderRead` and
  `ColorWrite`, exactly what `cmaa2_edges` already declares.
- **+55.4 MiB of render targets at 1920×1080 / 4× MSAA** (104.8 → 160.2),
  **0.23% of the card**; **+1.7 MiB at the golden gate's 320×200**.
- **Predicted +0.036 ms for the pass** (24.9 MB of traffic against CMAA2's
  measured 691 GB/s effective) and **+0.2–0.3 ms for the doubled multisampled
  colour image**, which four passes read or write per frame. So ~90% of the added
  cost is the format, not the pass; both plans presented it the other way round.
  Total graph ~0.4 → ~0.75 ms, 4.5% of a 16.7 ms frame. Falsify with
  `LOOM_GPU_TIMING=1` **at 1920×1080** — never at 320×200, where it is under the
  timestamp noise floor.
- **Nineteen of 25 golden references move; six must not.** The prediction is in
  the commit message and a scene moving out of tier is a finding.
- **A tonemap is resolution-independent, so the 320×200 gate measures the real
  thing.** That is not true of every candidate post effect and it is why this one
  is admissible where bloom is not.
- **Every AA number in this project becomes incomparable to its own history.**
  ADR 0010's MSAA curve, the blade-width sweep, the density-falloff rows, the
  `RAIN_MIN_PIXELS` salt metric and rain's 13.4% → 7.4% swing were all taken on a
  clipped 8-bit image. This project already carries the rule in as many words:
  *never compare two AA numbers across a change in colour or lighting*. The
  mitigation is to re-baseline `cargo xtask shimmer` in the same commit, record
  the new table here, and mark 0010's void the way `1062550` voided the numbers
  before it.
- **The MSAA resolve stops averaging destroyed values — and that cuts both ways.**
  A bright edge covering one sample of four goes from an on/off swing of ~83
  codes to ~207, because the clamp was accidentally acting as a tonemapped
  resolve. This is the textbook HDR+MSAA specular-aliasing exposure and it lands
  on grass against sky, wet-stone specular, water glitter and rain streaks — the
  exact list Phase 2's open question is about. **It is accepted, not dismissed**,
  and it is measured by the colour-invariant discriminator in the plan (the 4×/1×
  flicker *ratio*, not the absolute number). If that ratio worsens materially the
  mitigation is a per-fragment ceiling on the emissive/specular term, and that
  gets its own commit and its own number.
- **The two render paths get closer.** Fourteen pipeline-format arguments become
  one constant and `Msaa::new` loses its `format` parameter — the one place the
  paths were *required* to pass different values for the scene.
- **`Light::intensity` becomes a unit a scene can author**, and the 600× spread
  becomes an inconsistency that can be fixed rather than a fact of life.

## Consequences if rejected

`campfire.loom`'s arithmetic stays the model for every light in the engine; the
schema's documented range stays 600× wrong; fire keeps a ramp capped at 72% of
white and the reason stays true; and no scene with two bright things in it is
authorable.

## What stays outside the boundary, and why — so a third move has to argue

- **Bloom.** Its radius is a fraction of screen height, so a mip chain derived
  from resolution is 3 levels at 1080p and 1 at the gate's fixed
  `GOLDEN_SIZE = "320x200"` (`xtask/src/main.rs:338`). **The gate would validate a
  different effect from the one the human sees** — the same defect as the MSAA
  measurement bug and the shimmer-framing bug, a third time. That is a stronger
  reason than the pass count. Beyond it: a correct one is 11 passes, more than
  doubling the render graph; the graph has no subresource model
  (`crates/loom_render_graph/src/lib.rs:572-575` hardcodes
  `.base_mip_level(0).level_count(1)`), so it needs six separate images; and this
  project's own fire reference rejects the wash it produces
  (`scene.slang:1980-1983`: "deep amber with white over maybe two percent of its
  area"). **If the human still wants a glint on water after seeing the tonemap,
  that is a real request and gets its own ADR** — threshold 4–8× diffuse white,
  three mips, radius as a fraction of screen height, gate scenes rendered at a
  size where three levels exist, and the acceptance test that `materials`,
  `primitives`, `cave` and `ground` stay bit-identical.
- **Dither.** ±1 LSB is at or under `channel: 2`, so the gate literally cannot
  see it — correct or broken. Meanwhile `imagediff.rs:41-46` says the tolerance's
  only job is "survival across a driver update, which shifts a count or two"; a
  deterministic ±1 on every pixel halves that headroom frame-wide and the next
  `dnf update` pushes a large fraction past 2 against a 64-pixel budget. And
  banding is not made worse by this change: the sky is quantised to 8 bits
  exactly once today and exactly once after. **Revisit only if a gate scene
  visibly bands**, and then hashed on pixel coordinates alone, never on time.
- **Film grain.** Animated noise raises the flicker floor on every scene and
  destroys the three scenes scoring **exactly 0.000** — the control that makes
  `cargo xtask shimmer` trustworthy and the only instrument for the open Phase-2
  question. Generalises the rule the grass research reached from the other side:
  **no TAA means no animated noise anywhere.**
- **Chromatic aberration.** Colour fringing on a silhouette is what a broken
  normal map or tangent basis looks like; shipping it deliberately ships a
  permanent instance of the engine's own commonest bug signature, against a gate
  calibrated (ADR 0005) on a real regression whose worst channel delta is 4.
- **Motion blur.** Makes a frame a function of the previous frame's camera —
  verbatim `0010:79-82` — and velocity vectors mean re-evaluating grass, water
  and rain's procedural geometry at t−dt in the vertex shader.
- **Depth of field.** No authoring surface: `Camera` is `fov_y_degrees` and
  `active` (`components.rs:514-522`). And it renders part of every frame
  unreadable in an engine whose thesis is that the agent verifies its own renders.
- **Vignette, lens flare, lens dirt.** Lens defects in an engine with no lens. A
  vignette darkens exactly the region an agent checks for drift.
- **3D LUT grading.** 32,768 lines of floats is not diffable in the sense the
  property was written for, and it is a second opaque place where colour is
  decided.
- **SSAO — deferred, not refused, and it needs its own ADR.** The one item that
  is a missing *lighting term* rather than a look: `scene.slang:2540` records that
  grass fakes AO with a base darken because the renderer has none. Excluded here
  because letting it ride in on the tonemap's coattails is the erosion this
  document exists to stop, and because half the frame is grass drawn in the
  opaque block — 45,000 blades produce a depth hairball a hemisphere kernel will
  read as one enormous concavity, which is a design problem, not a tuning one.

## Status of the evidence

Format support: `vulkaninfo --show-formats` on this machine, 2026-08-15, GPU 0
`NVIDIA GeForce RTX 4090`. Range, precision and bandwidth: arithmetic from the
IEEE-754 half and sRGB definitions. Hue/saturation tables and the grey ladder:
computed from `scene.slang:1987`'s ramp under each candidate curve. The
bright-pixel census: decoded from the 25 files in `tests/references/`. **Every
timing figure is a prediction anchored on CMAA2's measured 0.042 ms and none has
been run.**
````

---

# The plan

Three slices, two `--bless` events. Ordered by image change per line.

---

## S1 — the frame is computed in float and collapsed once

**What.** `R16G16B16A16_SFLOAT` scene targets; one `tonemap` pass; `Environment.exposure`; the `pointLights` clamp. All four together, because an fp16 target with no curve is not a shippable intermediate (§0.7) and the clamp is a prerequisite for the format, not a follow-up.

**Why this order.** Everything else in this document is either a consequence of it or unauthorable without it.

**Files**

| path | change | ~lines |
|---|---|---|
| `assets/shaders/tonemap.slang` | **new** | +70 |
| `crates/loom_render/src/tonemap.rs` | **new** | +210 |
| `crates/loom_render/src/lib.rs` | `mod tonemap;`, `TONEMAP_SPV`, barrier list, one new test | +60 / −4 |
| `crates/loom_render/src/renderer.rs` | `HDR_FORMAT`; **7 pipeline sites** `:1021 :1029 :1035 :1042 :1051 :1065 :1077`; `Msaa::new` drops `format` (`:449`, `:866`); `loom.color_target` (`:831`) and its view (`:868`); the pass; `readback_source` at `:2154-2184` **and** the tuple at `:1710-1713`; teardown `:2361` | +80 / −40 |
| `crates/loom_render/src/viewer.rs` | **7 pipeline sites** `:306 :317 :320 :327 :340 :355 :363`; unconditional HDR scene image; the pass; UI retargeted; `recreate` `:1782`; teardown `:1865` | +100 / −50 |
| `assets/shaders/scene.slang` | `pointLights` clamp (`:300`) | +3 |
| `crates/loom_scene/src/components.rs` | `Environment.exposure` | +14 |
| `crates/loom_cli/src/main.rs` | `renderer.exposure = env.exposure` at the two build sites | +4 |
| `tests/references/*.png` + `MANIFEST.txt` | **19 move, 6 must not** | binary |

Nothing in `build.rs` — dropping a `.slang` into `assets/shaders/` is the whole build integration (`crates/loom_render/build.rs:29-50`), and `-fvk-use-entrypoint-name` (`:146`) keeps the two entry points named.

**Shader** — `assets/shaders/tonemap.slang`

```slang
// The one place the frame's range collapses (ADR 0019). Everything upstream is
// linear R16G16B16A16_SFLOAT with no ceiling; the attachment here is _SRGB, so
// the hardware does the encode exactly as it always has — what moved is WHERE,
// from the first fragment write to the last.
//
// Fragment, not compute, and that is a property of this renderer rather than a
// preference: no sRGB format supports STORAGE (cmaa2.rs:20-46).
//
// `Texture2D` + `Load`, no sampler: this is a 1:1 copy, there is nothing to
// filter, and a sampler is an object to create, name and destroy for no pixel.

[[vk::binding(0, 0)]] uniform Texture2D<float4> scene;

struct TonemapPush { float exposure; };   // Environment.exposure
[[vk::push_constant]] TonemapPush push;

// **Pure shoulder: identity below the knee, hue and saturation exactly
// preserved above it.** Measured against the published operators on fireRamp's
// amber rung (ADR 0019): ACES full and Khronos PBR Neutral both desaturate a
// fire FASTER than clipping does, which is the failure `fireRamp`'s cap
// (scene.slang:1977-1990) exists to prevent. This one holds hue to 0.3 degrees
// across six stops.
//
// Identity below KNEE is load-bearing, not incidental: it is what makes "which
// golden references move" computable from the existing PNGs before the change
// is written, and six of the 25 must not move at all.
//
// A flame's white core comes from `fireRamp`'s own top rung, not from a curve
// bleaching it. The art decides.
static const float KNEE = 0.76;

float3 shoulder(float3 c) {
    float peak = max(c.r, max(c.g, c.b));
    if (peak < KNEE) { return c; }
    float d = 1.0 - KNEE;
    return c * ((1.0 - d * d / (peak + d - KNEE)) / peak);
}

/// The same three vertices cmaa2_edges.slang:44 draws, for the same reason.
[shader("vertex")]
float4 tonemapVertexMain(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2(float((id << 1) & 2), float(id & 2));
    return float4(uv * 2.0 - 1.0, 0.0, 1.0);
}

[shader("fragment")]
float4 tonemapFragmentMain(float4 pos : SV_Position) : SV_Target {
    float3 hdr = scene.Load(int3(int2(pos.xy), 0)).rgb * push.exposure;
    return float4(shoulder(max(hdr, 0.0)), 1.0);
}
```

and in `scene.slang`, at the end of `pointLights` (`:300`):

```slang
    // **fp16 saturates at 65504, and `max(d2, 1e-4)` is a 1 cm floor with no
    // ceiling above it.** At intensity 30 and d = 2 cm the term is 75,000 —
    // +inf, which the MSAA resolve spreads and fog's lerp turns into NaN, the
    // one artifact `imagediff.rs`'s `worst` threshold exists to catch. Under
    // 8-bit fixed point this clamped harmlessly on store; it does not now.
    // 8192 is thirteen stops above white, far past anything the curve resolves.
    return min(sum, 8192.0);
```

**`tonemap.rs`.** `cmaa2.rs` with the sampler and the owned edge image removed, plus a 4-byte push range. Fields: one `DescriptorSetLayout`, one `DescriptorPool(max_sets(1))`, one `DescriptorSet`, one `PipelineLayout`, one `Pipeline`, and `(ldr, ldr_view, Option<Allocation>)` — **the LDR image has exactly one owner in both paths**, which is the rule `renderer.rs:411-420` states for the opaque pair ("an image that had to be remembered separately is an image that eventually is not — a dangling view under a live descriptor, with no validation message once the handle is reused").

Carried from `cmaa2.rs` verbatim in shape: `ldr_format` is a **constructor parameter** (`cmaa2.rs:131-137`); descriptors written in `rebind`, **never per frame** (`:309-318`); every fallible path after the first pipeline calls `self.destroy(...)` before returning (`:253-296`) or the object-tracking layer reports a leak instead of the real error; `names.set` on pipeline, image and view, because `create_image`'s `name` is the allocator's, not Vulkan's; `DONT_CARE`/`STORE`, no depth attachment, `TYPE_1`, `blend_enable(false)`, dynamic viewport/scissor, `cmd_draw(3,1,0,0)`; `destroy_ldr` split from `destroy` so resize reuses it (`:490-505`).

**`HDR_FORMAT`, and the fourteen sites**

```rust
/// The scene is summed in float and collapsed once, by the tonemap pass
/// (ADR 0019). Sixteen stops above diffuse white against zero, and a constant
/// 0.098% relative step against sRGB8's 1.0-6.8% across the band these scenes
/// paint in.
pub(crate) const HDR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
```

`COLOR_FORMAT` keeps its name and value and becomes what it always described: the format the finished, display-referred image lands in. **All fourteen scene-pipeline format arguments become `HDR_FORMAT`** (§0.3). `Msaa::new` loses its `format` parameter and its doc at `renderer.rs:439-441` is rewritten — the resolve destinations are HDR too, which is why `loom.scene_opaque` is HDR and water refracts unclamped radiance. `loom.color_target` **keeps `TRANSFER_SRC`** (§0.8).

The viewer's `format` field survives, feeding only `Tonemap`, `Cmaa2` and `Ui`, and the comment at `viewer.rs:296-299` is rewritten to say the opposite of what it says now.

**Viewer: the scene image becomes unconditional.** Today `loom.viewer_scene` is created inside `create_aa_target` (`viewer.rs:2065-2100`) and does not exist at `LOOM_CMAA2=0`, where the scene resolves straight into the swapchain (`viewer.rs:1141`). An sRGB8 swapchain cannot be an HDR blend target. So `create_aa_target` splits: `create_scene_target` returns the HDR image unconditionally; `Tonemap` owns the LDR image beside it; `self.aa` becomes `Option<Cmaa2>` alone.

**Passes and `Access`** — offscreen, immediately before `renderer.rs:2149`:

```rust
let ldr = graph.import("loom.scene_ldr", self.tonemap.ldr_image());
graph.pass(
    "tonemap",
    &[(color, Access::ShaderRead), (ldr, Access::ColorWrite)],
    move |d, cmd| unsafe { pass.record(d, cmd, exposure, width, height) },
);
```

`cmaa2_edges`/`cmaa2` take `ldr` where they took `color`; `readback_source` becomes `aa_target` or `ldr`, **never `color`** — and both `renderer.rs:2154-2184` and the tuple at `:1710-1713` must change, because getting only one right silently reads the un-post-processed frame, the bug shape `:1706-1709` already warns about. Viewer: identical declaration, then the `ui` pass (`viewer.rs:1523`) moves below it and targets `tonemap.ldr_view()`, then `cmaa2_edges`/`cmaa2` read `ldr`.

**No new `Access` variant.** Timer capacity is unchanged: `TIMED_PASSES = 12` (`renderer.rs:784`), offscreen 6 → 7, viewer 7 → 8 (§0.8).

**Barrier-test change** (`lib.rs:396-426`) — two rows added, one repointed:

```rust
    ("forward",     "loom.color_target"),
    ("forward",     "loom.msaa_color"),
    ("forward",     "loom.msaa_depth"),
    ("forward",     "loom.depth_target"),
    ("tonemap",     "loom.color_target"),   // COLOR_ATTACHMENT -> SHADER_READ_ONLY
    ("tonemap",     "loom.scene_ldr"),      // UNDEFINED        -> COLOR_ATTACHMENT
    ("cmaa2_edges", "loom.scene_ldr"),      // was loom.color_target
    ("cmaa2_edges", "loom.aa_edges"),
    ("cmaa2",       "loom.aa_edges"),
    ("cmaa2",       "loom.aa_target"),
    ("readback",    "loom.aa_target"),
```

`("cmaa2", "loom.scene_ldr")` is still correctly absent — read-after-read in the same layout emits nothing, as `lib.rs:417-421` already explains.

**Viewer resize** (`viewer.rs:1782`), keeping the existing order — device idle, swapchain, depth, `scene_depth.bind`, `Msaa` rebuilt, `water_textures.bind` (do **not** move this; `:1770-1780` records that binding before the rebuild pointed a descriptor at views destroyed on the next line) — then:

```rust
destroy(scene_view); destroy(scene_image); allocator.free(scene_alloc);
let (scene_image, scene_view, scene_alloc) = create_scene_target(.., extent)?;
unsafe { self.tonemap.rebind(&self.device, alloc, &self.names, scene_view,
                             extent.width, extent.height)?; }
if let Some(aa) = self.aa.as_mut() {
    unsafe { aa.rebind(&self.device, alloc, &self.names,
                       self.tonemap.ldr_view(), extent.width, extent.height)?; }
}
```

`Tonemap::rebind` destroys and recreates the LDR image at the new extent and rewrites its one descriptor. Pipeline, layout, pool and set survive.

**Scenes and gates.** No scene added. `GOLDEN` unchanged in membership; **19 references re-blessed, 6 must not move.**

**Test and the mutation.**

```rust
/// A tonemap wired correctly and doing nothing passes every other check in this
/// suite — validation stays silent, the graph places its transitions, the image
/// looks right. This is the check that can tell the difference.
#[test]
fn the_tonemap_compresses_and_leaves_mid_tones_alone() {
    // One render of a scene with a point light at intensity 40 over a sphere.
    // (a) strictly fewer pixels at 255 in every channel than the same scene
    //     rendered at LOOM_TONEMAP=0, holding `exclusive()` across both;
    // (b) at exposure 0.5 the frame is strictly darker than at 1.0;
    // (c) every pixel of `primitives` whose max channel is <= 226 is unchanged
    //     from the LOOM_TONEMAP=0 render.
}
```

- **Mutation for (a):** `shoulder` returns its argument → images match, white counts equal.
- **Mutation for (b):** hard-code `1.0` in `record` instead of reading the field → fails.
- **Mutation for (c):** restore PBR Neutral's black-point offset → `primitives`' mid-tones move, and `cave`, `meadow`, `smoke`, `windy`, `grass_slope` fail `cargo xtask image`. **This is the mutation the whole design is built to catch, and it is the mistake both plans made.**
- **Mutation for the barrier list:** drop `(ldr, ColorWrite)` from the declaration → the row disappears *and* `cargo xtask validate` reports the image left in `UNDEFINED`.
- **Mutation for the clamp:** delete `min(sum, 8192.0)`, author a light inside geometry → NaN pixel, `worst` fails.

**Cost.** Tonemap pass **+0.036 ms** at 1080p (24.9 MB against CMAA2's measured 691 GB/s effective). Doubled `loom.msaa_color` read/written by four passes: **+0.2–0.3 ms**. VRAM **+55.4 MiB** at 1080p/4×, **+1.7 MiB** at 320×200.

**Risk, highest first.**
1. **The viewer.** It grows an image it has never had and reorders a pass, at the seam the brief says has cost three defects. **Build the viewer half first, not last**, and finish the commit with a manual `loom compare` between `loom render --size 1440x900` and a screenshot of `loom run` at the same size — there is no automation for a windowed path and pretending otherwise is how the MSAA measurement bug survived four phases.
2. **Missing one of the fourteen pipeline formats** — a validation error on every draw of that pipeline, caught by `cargo xtask validate` over 41 scene runs including five `loom run --edit` scenes.
3. **The clear colour.** `renderer.rs:2673-2675` clears to `[0.05, 0.06, 0.08, 1.0]`, interpreted in the format's space; on fp16 it stays linear and is then tonemapped. Visible only where nothing is drawn, and the sky is a full-screen draw. A border on any reference is this.
4. **MSAA specular aliasing gets worse** — accepted, measured separately (see The measurement).

---

## S2 — the art re-authored to physical values

**What.** Spend the range S1 bought, one line and one reason at a time.

**Why this order.** Only after the curve exists is any of it authorable, and a second bless is only readable against a settled baseline.

**Files:** `assets/shaders/scene.slang`, `assets/shaders/rain.slang`, `crates/loom_scene/src/components.rs:84`, and the `.loom` files below.

| what | where | now | after |
|---|---|---|---|
| `fireRamp` peak cap | `scene.slang:1984-1990` | `(0.72, 0.640, 0.480)` | uncapped; the comment explaining the cap becomes the comment explaining its removal |
| campfire `Light.intensity` | `campfire.loom:131` | 1.35 | 22, and the ten-line clip-arithmetic block **deleted**, not amended |
| `Light::intensity` doc | `components.rs:84` | "typically 100-800" | the real range, derived from `I·albedo·h/d³` with a worked example |
| the 600× spread | `blockout:91` 800, `office:50` 800, `workshop:65` 700, `explosion:194` 300, `proving_ground:178` 260 | — | re-authored **after measuring**, not on inference |

**Measure before re-authoring the spread.** `blockout` is in `SCENES` (`xtask/src/main.rs:42`) but **not in `GOLDEN`** — 25 entries, absent — so no pixel comparison has ever looked at it. At 800 with a 3 m ceiling light the term is ~90× the clip and that room is almost certainly flat white. Render it, count pure-white pixels, and either add it to `GOLDEN` in this slice or say in the commit why not.

**Not touched, and the ADR says so:** fire stays a level set; `WATER_CLARITY_ROUGH`, `RAIN_MIN_PIXELS`, `WATER_FOAM_FINE_RANGE` stay. **And the sun disc, `CLOUD_LIT`, `WATER_FOAM_ALBEDO` and `RAIN_STREAK_BRIGHTNESS` stay too** — they were chosen against the clamp, they still look right, and moving them re-blesses references for no reported symptom. Widening the range is not the same as spending it. Their comments get corrected to say "chosen" rather than "chosen because it clips".

**Scenes and gates.** Expect `campfire`, `explosion`, `proving_ground` and any `Light`-authoring scene added to `GOLDEN` to move. **Predict the set in the commit message before running.** Second `--bless`.

**Test and the mutation.** No new test. The S1 test's clause (a) covers the ramp; the gate is the point. **Mutation:** re-cap `fireRamp` → `campfire` and `explosion` move back.

**Cost.** Zero ms, zero MB.

**Risk.** This is where "the picture got worse" becomes possible for the first time. A campfire at 22 under a curve is a judgement, not a proof. `tools/watch.sh campfire` and a 1920×1200 still, before and after, looked at by a human.

---

## S3 — `assets/test/lanternhead.loom`, the showcase

Full description in **The showcase scene** below. Files: one `.loom`, three lines in `xtask/src/main.rs` (`SCENES` `:41`, `GOLDEN` `:155`, the CPU-budget array `:935`), one new reference. Existing references unmoved — **that is this slice's acceptance test**. Cost: predicted 3–6 ms debug CPU against `CPU_BUDGET_MS = 30.0`, ~7.5 s voxel bake at load, under 0.5 ms GPU. Risk: six framing/bake checks listed there, none of which I could run.

---

# What has to be re-tuned

Everything below was tuned against a response that clamps at 1.0. **Under a pure shoulder, anything whose value stays below linear 0.76 is untouched** — which is why this list is short and why the curve was chosen that way. If PBR Neutral or ACES had been taken, this list would be every colour constant in the engine.

| constant / scene | where | status under the shoulder | action |
|---|---|---|---|
| `fireRamp` peak `(0.72, 0.640, 0.480)` | `scene.slang:1984-1990` | below the knee → renders identically | **uncap in S2**; the reason for the cap is gone |
| `campfire.loom` `intensity = 1.35` | `campfire.loom:131` | renders identically; the arithmetic that produced it is void | **re-author to 22 in S2** |
| `Light::intensity` doc + default | `components.rs:84`, `:95` | ~600× wrong against the only working scene | **rewrite in S2** |
| `blockout` 800 / `office` 800 / `workshop` 700 | `:91`, `:50`, `:65` | never gated; almost certainly flat white today | **measure, then re-author.** Consider `GOLDEN` |
| `explosion` 300, `proving_ground` 260 | `:194`, `:178` | in `GOLDEN`; whatever happens is visible in the diff | measure |
| sun disc `0.9`, glow `0.18` | `scene.slang:465-466` | below the knee → identical | **leave.** Comment corrected only |
| `CLOUD_LIT = 0.78` | `scene.slang:493` | below the knee → identical | **leave** |
| `WATER_FOAM_ALBEDO 0.72–0.78` | `scene.slang:3013` | below the knee → identical | **leave** |
| `RAIN_STREAK_BRIGHTNESS = 0.42` | `rain.slang:168` | below the knee → identical | **leave** |
| ambient/sun "without washing out" | `scene.slang:348` | below the knee → identical | **leave** |
| `WET_SMOOTH = 0.08` wet-stone specular | `scene.slang:1146`, `:1648` | specular *can* now exceed 1.0 where it previously clamped | **watch `homestead`** — its own file says the wet bank "is on the edge of blowing out" |
| **`cargo xtask shimmer`: every number** | ADR 0010, CLAUDE.md phase notes | MSAA curve, blade-width sweep, density-falloff rows, CMAA2 table | **void.** Re-baseline in S1's commit, record in ADR 0019, mark 0010's void |
| **rain's 13.4% → 7.4% brightness swing** | CLAUDE.md, P4 notes | taken on a clipped image | void; re-measure if rain is touched |
| `RAIN_MIN_PIXELS = 2.5` salt metric | `rain.slang:153` | sub-pixel coverage, not range | **leave** — claiming it would over-state the evidence |
| `WATER_CLARITY_ROUGH = 0.22` | `scene.slang:2950` | roughness-aware Fresnel; correct physics | **leave** |
| `imagediff.rs` `worst: 72` | `:53` | calibrated when bright pixels pinned at 255 and stayed there; they are live values now | **expect spurious `worst` churn** on `campfire`, `explosion`, `ocean`, `beach`. **Do not widen it** — `:47-50` records that a threshold of 8 hid a real one-line shader change |

---

# The showcase scene

`assets/test/lanternhead.loom`. **The shot: a brazier burning on a wet stone quay at last light, a lantern line receding into the rain, a boat riding in a glassy harbour that shows its own bed, and open sea behind it.**

**Dusk, not night, and one shader line settles it.** `grassFragmentMain` (`scene.slang:2521-2554`) is `albedo * (ambientStrength()*occlusion + sunStrength()*wrapped*occlusion)` — no `pointLights`, no `sunVisibility`, no wetness, and `pointLights` is called from exactly one place, `scene.slang:1709`. A scene authored the way `campfire.loom` is (`sun_strength = 0`, `ambient = 0`) renders its grass black and its sea black. A night showcase discards grass, water, weather and vegetation to demonstrate two systems `campfire.loom` already demonstrates and is already golden.

Five things in this frame are above diffuse white and every one is currently clamped, capped in the shader, or authored down by a factor of twenty: the flame core, the brazier's light pool, the sun's glitter path, wet stone under a low sun (`WET_SMOOTH = 0.08` is a near-mirror), and the sun disc. **It is not renderable before S1 and correct after it.**

**Three things critique 2 was right to delete, and I deleted them:**
- **The trees.** `trees.loom` is in neither `SCENES` nor `GOLDEN` — the `file.obj#Object` path and per-tree atlas binding have never been checked by any of the four green checks — and there is no alpha cutout (`scene.slang:1518` takes `sampled.rgb`, no `discard` anywhere in `fragmentMain`), so leaf cards are opaque quads. Landing an ungated import path in the same commit as a new golden reference means nobody can tell whether a wrong reference is the curve or the atlas. **Add trees as their own commit, after.**
- **`Scatter`.** `crates/loom_cli/src/main.rs:1815` sets `material: u32::MAX` and `:1792` gives a whole field one flat albedo. Untextured cylinders at scale.
- **Authored puddles.** Arithmetic: `puddleMask` divides a hollow measured over `PUDDLE_REACH = 1.1` by `PUDDLE_FULL = 0.16` (`scene.slang:1281-1305`), so a subtracted sphere needs `R ≤ 3.78 m` to saturate, and the `PUDDLE_SLOPE = 0.12` gate then leaves a flat centre 0.91 m across; a dish shallower than one voxel is under the SDF quantisation step, which is why `puddles.loom:117-119` runs at 0.25 m. **The wet sheen needs no concavity** and is what this scene uses.

**One thing critique 2 was wrong about, and I kept it:** the smoke plume. Particles are unlit by design (`particleFragmentMain`, `scene.slang:2051-2137`) and every scene authors their colour to suit — `smoke.loom` is in `GOLDEN` on exactly that basis. It is authored, not broken. Its ramp is set for a dusk ambient.

**Sun in frame:** kept, at 9.8° elevation and 17.7° off-axis. It is the shot the ADR exists to justify and it is one direction vector, trivially moved after looking at the render. Flagged as a thing to check, not a thing to pre-emptively design around.

## Node list

```toml
# lanternhead.loom — a fishing quay under a headland, low sun, in the rain.
# The standing shot for the post-process stack, and unauthorable before it:
# the lights below are physical, and the ceiling on 8-bit fixed point is ~1.5.
#   loom render assets/test/lanternhead.loom --sim 2400 \
#     --out lanternhead.png --size 1920x1200

[scene] format = 1 ; id = "<new uuid>"

[[asset]] ground_soil_albedo / ground_soil_normal / ground_rock_albedo / ground_rock_normal
# No fire_flipbook alias: scene.slang:2066 converts EVERY additive sprite when declared.

Lanternhead        Environment  sun_direction=[0.30,0.17,-0.94]  sun_strength=1.9
                                sun_color=[1.00,0.72,0.42]       ambient=0.34
                                sky_zenith=[0.055,0.075,0.135]   sky_horizon=[0.44,0.34,0.26]
                                fog_density=0.005  fog_falloff=0.045
                                cloud_cover=0.45   cloud_scale=260.0
                                exposure=1.0          # first scene to author it
                   Wind         direction_degrees=155.0 speed=7.0 gustiness=1.4
                                turbulence=1.0 ground_drag=0.45
                   Rain         intensity=5.0         # no duration: still falling

Ground             VoxelVolume  voxel_size=0.32  chunks=[5,2,5]
   1 sphere c=[22.0,-82.0,30.0] r=88.3              union    sea bed
   2 box    c=[7.5,5.2,18.0]  h=[7.5,4.0,10.0]      union    rock plinth, top 9.2
   3 sphere c=[7.5,-6.0,18.0] r=20.5                union    grassed dome, summit 14.5
   4 box    c=[37.0,5.4,32.0] h=[11.0,4.0,12.0]     union    quay, deck 9.4
   5 sphere c=[21.0,-12.7,32.0] r=22.0              union    slipway, 1:2.78
   6 sphere c=[26.0,8.6,34.0] r=2.2                 SUBTRACT quay-face scoop
   7 box    c=[39.5,10.7,22.425] h=[3.5,1.3,0.425]  union    shed back wall
   8 box    c=[39.5,10.7,28.575] h=[3.5,1.3,0.425]  union    shed front wall
   9 box    c=[36.425,10.7,25.5] h=[0.425,1.3,3.075] union   shed left wall
  10 box    c=[42.575,10.7,25.5] h=[0.425,1.3,3.075] union   shed right wall
  11 box    c=[39.5,12.45,25.5] h=[4.1,0.45,4.1]    union    shed roof, 0.6 m overhang
  12 box    c=[38.6,10.35,28.575] h=[0.75,0.95,0.75] SUBTRACT doorway 1.5 x 1.9 m
                   Material     albedo=[1,1,1] roughness=0.92 porosity=0.78
                                uv_scale=[0.5,0.5] triplanar=true
                                albedo_map=ground_soil_albedo normal_map=ground_soil_normal
                   Material.layer albedo=[1,1,1] roughness=0.72 porosity=0.14
                                uv_scale=[1.1,1.1] slope=0.62
                                albedo_map=ground_rock_albedo normal_map=ground_rock_normal

Harbour            WaterBody    kind="ocean" surface_height=8.0 density=1025.0 drag=60.0
                   .waves       attenuation_depth=6.0  max_height=1.15
                                26.0 / 0.55 / 0.35 / [ 0.20, 1.0]
                                15.0 / 0.30 / 0.45 / [-0.30, 1.0]
                                 9.0 / 0.16 / 0.50 / [ 0.50, 1.0]
                                 5.3 / 0.08 / 0.55 / [-0.20, 1.0]

GrassCrown    pos=[7.5,0,16.0]  Grass half_extent=[5.5,6.0] density=150 height=0.42
GrassShoulder pos=[8.0,0,24.0]  Grass half_extent=[4.5,3.5] width=0.020
                   (both)       slope_cutoff=0.62 clump_facing=0.75 clump_colour=0.4
                   Material     albedo=[0.24,0.36,0.12] roughness=0.75

Brazier    pos=[30.5,9.62,35.5] scale=[0.42,0.22,0.42]  MeshRenderer cylinder
                   Material     albedo=[0.09,0.08,0.07] metallic=0.6 roughness=0.55
Flame      pos=[30.5,10.35,35.5] ParticleEmitter burst=1 rate=0 lifetime=110.0
                                size=[1.4,1.4] alpha=[1,1] color_*=[1,1,1]
                                additive=true flame=true   # all motion terms 0
Smoke      pos=[30.5,11.0,35.5] ParticleEmitter rate=34 lifetime=2.2 speed=1.5
                                spread_degrees=24 radius=0.26 gravity=1.5 drag=0.9
                                turbulence=1.6 turbulence_scale=0.3 wind_response=4.0
                                size=[0.40,2.2] alpha=[0.34,0.0] seed=8837
                                color_start=[0.20,0.19,0.18] color_end=[0.46,0.46,0.49]

Brazierlight pos=[30.5,10.0,35.5]  Light intensity=30.0 color=[1.00,0.50,0.17]
LanternA/B/C pos z=32.0/24.0/20.5, y=11.9   Light intensity=12.0 color=[1.00,0.78,0.45]
ShedLamp     pos=[39.5,10.9,25.5]  Light intensity= 8.0 color=[1.00,0.80,0.52]
PostA/B/C    same z, y=10.7, scale=[0.09,1.3,0.09]  MeshRenderer box
MooringA/B/C [23.5,7.70,34.0] [19.5,7.70,28.0] [21.0,7.55,23.0] scale=[0.13,1.5,0.13]

Dinghy  pos=[24.0,8.2,30.5] rot_euler=[0,16,0] scale=[0.9,0.35,2.4] MeshRenderer box
                   Material     albedo=[0.34,0.26,0.17] roughness=0.80
                   RigidBody    dynamic=true mass=1800.0     # 300 kg/m3, settles
                   Buoyancy     (all defaults: four corner pontoons)

Camera  pos=[27.2,11.1,43.0] rot_euler=[-7.0,0.0,0.0]  Camera fov_y_degrees=58.0
```

**Why each op is what it is.** The bed is a **sphere** so it is 5.02 m deep at r = 24 m — exactly `WATER_TINT_DEPTH` — so the tint saturates *before* the depth grid ends and the grid's boundary is not drawn as a hard rectangle in the sea (`shore.loom`, `homestead.loom:218-224`). Ops 2 and 3 **together** are what give `GroundLayer` a job: a dome alone never exceeds 46.9° above water, so a rock layer matched to `Grass::slope_cutoff` would never appear; the plinth supplies vertical faces, the dome supplies grass, and `slope_cutoff` and `layer.slope` are both 0.62 because `components.rs:1570` pins them equal and `ground.loom:139-141` explains that two thresholds on one hill draw two concentric rings. The slipway is the only gentle water's edge because `beach.loom:62-71` gives the rule — a one-voxel bed step becomes a horizontal stair of `voxel/slope`, 0.89 m here, which the swash covers. The scoop is **subtracted**: union spheres read as wet black domes (`homestead.loom:263-266`). The shed is voxel CSG, not a mesh box, because `crates/loom_rain/src/collide.rs:24` bakes voxel volumes ∪ static `BoxCollider`s and `wetGate` marches `loom_ground_height` — a mesh shed shelters nothing. `lifetime = 110.0` is long but **bounded**: `validate` warms every emitter for two lifetimes (`campfire.loom:148-150`). Yaw is **0** so the composition does not depend on a rotation convention nobody has rendered.

**One `WaterBody` reading as two:** `attenuation_depth = 6.0` over a 1.7 m basin flattens the swell inside the harbour so refraction shows the bed and the caustics play on it, while beyond the volume the depth grid returns its sentinel and the open sea carries the full swell. `homestead.loom:57-66`'s trick, and the only option — `World::water()` takes the first body and the schema has no extent.

**Two substances under one rain** is most of what sells wet: soil at `porosity = 0.78` darkens to 0.678 of dry at soak 0.589; rock at 0.14 barely moves (`components.rs:199-201`).

**Gates.** `SCENES` **and** `GOLDEN` at `--sim 2400`, **and** the CPU-budget array at `xtask/src/main.rs:935`, which today measures exactly two scenes. 2400 ticks = 40 s because three things coincide: it is exactly `WET_COVER_WINDOW` (`crates/loom_rain/src/lib.rs:121`) so `cover_recent`'s three taps have a full window; `film ≈ 1.000` and `soak = 0.589`; and the boat has settled. Fallback `--sim 1200`.

```bash
loom sim assets/test/lanternhead.loom --ticks 2400 \
  --assert "rain@39.5,9.9,25.5.exposure < 0.05" \   # in the shed: sheltered
  --assert "rain@39.5,9.9,25.5.rate   < 0.05" \     # and therefore dry
  --assert "rain@30.5,9.5,35.5.exposure > 0.99"     # by the brazier: open sky
```

The exposure pair is what makes the rate pair mean anything — without it nobody can tell whether cover or shelter did the work (`squall.loom:39-44`). **A fourth assertion on the brazier's rain rate cannot be written blind:** at `cloud_scale = 260` and 7 m/s the deck's position at tick 2400 is a measurement to take and pin. If the quay is under a gap, nudge the tick, not the cover.

**Presentation.** `--size 1920x1200` — **aspect 1.6, identical to the gate's 320×200**, so the reference frames exactly what the human sees (`puddles.loom:37-54` is the incident that rule comes from). Plus `SIZE=960x600 FRAMES=20 GRID=4x5 STEP=8 WARMUP=2400 tools/watch.sh lanternhead`, because half this scene is motion. Plus `cargo xtask flythrough`, free from `SCENES`.

**Six things to verify before typing it in — none of which I could run:** the ~7.5 s bake against an 8 s fallback to `voxel_size = 0.4, chunks = [4,2,4]`; the cloud's position at tick 2400; the framing at 320×200 including whether the plinth's clip at x = 0 is out of frame; that the slipway's waterline circle (r = 7.45 about `[21,·,32]`) swallows neither the quay's near corner nor the camera's feet; whether the far half of each grass field reads as ploughed earth (if so the ground wants a greener tint — a scene rule, not an engine one); and that `GroundLayer` at 0.62 puts rock on the plinth faces and **nowhere** on the dome.

---

# The measurement

Four numbers, and the design of each is aimed at the failure this project keeps hitting — an instrument that stops containing its subject.

**1. The bottom of the range — the prediction, computed before the code exists.**
`cave`, `grass_slope`, `meadow`, `primitives`, `smoke`, `windy` contain **zero pixels above sRGB code 226** and must be **bit-identical**; `materials` has 3, under the 64-pixel `fraction` gate, and must pass unchanged. **If any of them moves, the curve is touching values below the knee.** This is what distinguishes the shoulder from PBR Neutral, which moves all 25 and predicts nothing.

**2. The top of the range — pure-white pixels, before and after, per scene.**
The measurement that diagnosed the original bug ("3.71% of the frame at pure white"). Present it as a table in S1's and S2's commit messages; the census above is the "before" column. The subject-containment argument is arithmetic rather than assertion: `proving_ground` has **18,345 clipped pixels, 35.6% of the frame**, `homestead` 852, `explosion` 803. The gate already looks at the thing.

**3. Whether MSAA still works — the discriminator, chosen to be colour-invariant.**
Absolute `cargo xtask shimmer` numbers are void across this change; CLAUDE.md's rule says so and this is the largest colour change the engine will make. So the question — *did unclamping the resolve make sub-pixel bright geometry less stable?* — is answered by the **4×/1× flicker ratio measured entirely within one build**. Today `meadow` is 3.888 at 1× and 2.712 at 4×, ratio **0.697**. Re-measure both under the new pipeline. **If the ratio rises materially, MSAA got less effective** and the mitigation is a per-fragment ceiling on the emissive/specular term, in its own commit with its own number. The three 0.000 controls must stay at 0.000.

**4. Cost — `LOOM_GPU_TIMING=1` at 1920×1080, never at 320×200.**
Read the `forward`, `water`, `tonemap` and `cmaa2` rows, never the `graph` total (it includes the PCIe-bound readback). Prediction to falsify: tonemap 0.036 ms, whole graph 0.4 → 0.75 ms.

---

# What we are not doing

| | trigger that would change it |
|---|---|
| **Bloom** | The human sees the tonemapped fire and the glitter path and **still** asks for a glow. Then its own ADR — and it must answer the gate problem first: a resolution-derived mip chain is 3 levels at 1080p and **1 at the fixed 320×200 gate**, so the gate would validate a different effect from the one shipped. That is the MSAA-measurement bug a third time and it is the blocking objection. |
| **Dither** | A gate scene visibly bands. Then hashed on pixel coordinates only, never time. Not before: it is invisible under `channel: 2`, it halves the driver-noise headroom frame-wide against a 64-pixel budget, and the sky is quantised to 8 bits exactly once today and exactly once after. |
| **A `LOOM_TONEMAP` switch as a shipped uniform** | Never as a `uint` in the fragment shader — that is a permanent dead branch in the hottest full-screen shader for a measurement. `LOOM_TONEMAP=0` is a Rust-side `Option<Tonemap>` on the `cmaa2::requested()` pattern (`cmaa2.rs:107-110`), used by the S1 test and then left alone. |
| **A `MANIFEST.txt` clipped/crushed census** | *Rejected, against critique 3's insistence.* Once the references are blessed against the curve, stubbing it to `saturate()` moves 19 of them and fails — the gate already catches it. What the census buys is reviewability of one re-bless, and the offline prediction supplies that. Add the column when a second re-bless needs reviewing and the prediction is not enough. |
| **A new clipping fixture in `GOLDEN`** | *Rejected.* `proving_ground` already carries 18,345 clipped pixels and is already golden. Rung 2. |
| **Raising `TIMED_PASSES`** | *Rejected.* 12 capacity, 7 and 8 passes after this. Its doc comment ("declares two passes") is stale — fix the comment. |
| **A source-text `both_render_paths_declare_the_same_passes` test** | *Rejected*, and critique 1 is right about why: it compares pass *names*, so it passes while a pipeline is built for the wrong format, an `Access` is inverted, a `rebind` is missing from `recreate`, or a different image is passed to the same-named pass — and none of the three historical defects was a name. A gap with no test is safer than a gap with a test that always passes. The real guard is the fourteen format arguments collapsing to one constant, `Msaa::new` losing its parameter, and `cargo xtask validate` running `loom run --edit` on five scenes. |
| **SSAO** | Its own ADR, and an answer to what a 45,000-blade depth hairball does to a hemisphere kernel. The one deferral that is a missing lighting term rather than a look. |
| **Auto-exposure, motion blur, grain, CA, vignette, DOF, LUTs, `contrast`/`saturation` knobs, compute post passes, HDR presentation** | Nothing. Refused with reasons in ADR 0019 so the next person argues against a written decision. |
| **A swapchain readback to diff the two paths** | The next defect at that seam. It is a real feature and should arrive with the bug that justifies it. |

---

# The single biggest risk

**Not the format, the pass, the cost or the re-bless. It is that moving the MSAA resolve from clamped to unclamped makes sub-pixel bright-geometry aliasing measurably worse — a bright edge covering one sample of four goes from an on/off swing of ~83 codes to ~207 — and it lands on grass against sky, wet-stone specular, water glitter and rain streaks, which is the exact list Phase 2's still-open exit criterion is about.** Both source plans assert this change *improves* AA and both scheduled the shimmer re-baseline inside the same commit, so the regression would arrive already un-attributable, in a project that has already lost a night to an instrument that quietly stopped seeing its subject.

The mitigation is not to avoid it — the clamp was destroying energy and that is the whole point of the change — but to measure it with an instrument the colour change cannot fool. That is the **4×/1× flicker ratio within one build** (measurement 3): today 2.712/3.888 = 0.697, re-measured after, and a material rise is the finding. It is one extra `shimmer` run and it is the difference between an accepted consequence and a silent one.