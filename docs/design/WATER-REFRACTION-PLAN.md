# Water refraction — the plan

**Status: designed, not built.** Slice 0 has been run and its abort condition
did not fire; the two prerequisite commits are in (`c2d611a` blessed
`water_crate` and `splash` into `GOLDEN` *before* any topology change, and
`7f04a71` fixed the dead `ms_depth` STORE so slice 1's timing delta is measured
against a clean baseline). Slice 1 onward is unbuilt.

Produced by a twelve-agent review — three audits, three research passes, two
competing designs, three adversarial critics, one synthesis — against the state
of the tree at `7f04a71`. **Part 0 is the most valuable section**: it is the
list of things the two designs got wrong, each checked against source, and it
killed one plan's entire path-length formulation.

## Slice 0 result, run 2026-08-15

The abort condition was "the turbidity knob alone fixes `ocean`". It does not,
and the reason is worth keeping: raising `WATER_BACKSCATTER` does move `ocean`'s
water toward the reference hue — R/B 0.804 -> 0.718 at 10x, 0.694 at 30x against
a photographic target of 0.62 — but it buys that blue by making the water
physically wrong. At 30x, `R_inf` in blue is 0.33 x 0.0525/(0.013 + 0.0525) =
**0.264**, and real open ocean is 2-5% reflectance. Even 10x gives 0.189, which
this document's own Part 0 calls "Caribbean shallows". The knob is a cheat, not
a fix. **Proceed.**

## One constraint the agents could not derive

None of the twelve can see an image, so this was measured by hand and is a hard
acceptance test for slice 2:

`shore.loom`'s shallow water was turquoise before commit `0651775` and is grey
after it. In a fixed crop of the shallow band (190x55+25+108 at 320x200):

| | R | G | B | **G-R** | R/B |
| --- | --- | --- | --- | --- | --- |
| before `0651775` | 86 | 124 | 134 | **38** | 0.644 |
| after | 68 | 82 | 93 | **14** | 0.737 |

Turquoise is green-excess-over-red and it dropped 63%. The cause is that
`0651775` deleted `body * 6.0 * pow(dot(-view, sunDir), 3)` — a term that was
wrong in mechanism (it glowed troughs as brightly as crests) but was carrying
the entire shallow-water look — and replaced it with one gated on `crest`.

**That term was a fake stand-in for refraction.** Shallow water is bright
because light reflects off the bed and comes back up through it, which is
exactly what refraction supplies. So:

> **Refraction must restore `G-R` in that crop to >= 38 with no compensating
> constant.** If it does not, refraction has not actually replaced what was
> deleted, and the honest fix is to reinstate an ambient-driven body term for
> shallow water rather than leave `shore` worse than it was.

Note also that `shore`'s shallow band measured **R/B 0.644 before the
regression** — essentially the NOAA storm-sea reference's 0.62. The target hue
is reachable, because we had it.

---

No repository file was created, modified, deleted or `git`-ed. Everything below is read from source in this worktree; I ran no experiments and needed no scratch directory.

---

## Part 0 — Corrections that change the plan

**1. `T = 0` at the 100 m sentinel is false in blue. Plan 1's path length is dead.**
`scene.slang:2504` — `WATER_EXTINCT = float3(0.341, 0.058, 0.013)`, and the comment two lines above says it in words: *"Red is gone in three metres, blue survives fifty."* `exp(-0.013 × 100) = 0.2725`. Plan 1 evaluated red, called it `T`, and built its whole "no guard needed" argument on it. On `ocean` (camera −7°, `ocean.loom:194`) `cosT ≈ 0.67–0.75`, so `path = 100/cosT ≈ 133–150` and **T_blue = 0.14–0.18** against sky at `[0.58, 0.66, 0.72]` (`ocean.loom:55`). `ocean` and `squall` move over most of the frame, and Plan 1's own stated falsification (*"if `ocean` moves, the reconstruction is wrong"*) fires on run one. This is the identical error the brief says two previous critics caught — 27% is `exp(-0.013 × 100)`. **Plan 1's `in.depth` formulation is deleted, not tuned.** Raising the clamp is not a repair: `scene.slang:2489-2496` clamps at 100 because the off-grid sentinel is 1e9 and interpolating it perspective-divides into negatives, and a legitimate 100 m lake would then attenuate identically to "no bed".

**2. Plan 2's sentinel safety is real but for the wrong reason, and past 1000 m it inverts.**
Far plane is 1000 m (`renderer.rs:2598`, `Mat4::perspective_rh(fov, aspect, 0.1, 1000.0)`) while `WATER_HORIZON = 50000.0` (`scene.slang:2246`), and the vertex shader deliberately drags the skirt inside the clipper (`scene.slang:2478-2480`, *"Held inside the far plane, which is what lets the skirt reach 50 km through a projection that ends at 1000 m"*). So for every water fragment beyond 1000 m, `distance(eye, bg) − distance(eye, worldPos)` is **negative**, `max(…, 0)` gives 0, `T = 1`, and the sea past the far plane renders as a transparent hole onto the sky. Plan 2's `max(…, 0.0)` fails toward full transparency. **Fix: guard on the raw depth sample, not on the subtraction** — `if (d >= 1.0) { T = 0; }`. Sky writes no depth (`renderer.rs:3410-3412`), so one test covers the sentinel scenes, the 50 km skirt, and any future far-plane change. It is also the branch that makes refraction free on `ocean`/`squall`.

**3. The seabed is lit through one path length in both plans. It needs two.**
`behind` is the bed shaded **in air** by the mesh pass. The light that reached it crossed the column downward first. Standard shallow-water form (Lee et al.): `T = exp(−c(H/cos θ_down + L_view))`. Both plans have only `L_view`. On `shore` (bed 6 m, sun elevation `sun_direction = [0.16, 0.34, −0.93]`, `shore.loom:38`) the down leg is ~17.6 m and red drops by a further `exp(−6.0) = 0.0025`. Omitting it ships bright, neutral, milky shallows — the human's complaint arriving from the other side — and `--bless` will write it into `tests/references/` as truth. **The shader already contains the term**: `thick` at `scene.slang:2706-2707` divides by `max(sunDirection().y, 0.15)` for exactly this reason, four lines from where the new code goes. Neither plan noticed.

**4. Double fog is ~18% on `shore`, not "well under a percent".**
`fog_density = 0.004` (`shore.loom:44`); water spans ~15–50 m from a camera at `[25, 10.4, 42]`. `1 − exp(−0.004 × 50) = 0.181`. The mesh path fogs `behind` over eye→bg (`scene.slang:1621`, `lit = lerp(lit, fogColor(-view), fog)`) and `scene.slang:2757` fogs the composite again over eye→surface, and the bed is 1–2 m behind the surface, so the overlap is nearly the whole path. Applied twice, `0.18` becomes `1−(1−0.18)² = 0.33`. Both plans said "name it, don't fix it". It is two lines and it is 15 percentage points on the payoff scene. Unwind it.

**5. Both plans put the new colour binding on set 2. Both consequences are real and a fourth set fixes both.**
Set 2's binding 0 is `sceneDepth` pointing at `loom.depth_target` in `SHADER_READ_ONLY_OPTIMAL` (`scene_depth.rs:83`), and the water pass declares that same image `Access::DepthResolve` → `DEPTH_ATTACHMENT_OPTIMAL` (`renderer.rs:2359`). Binding it inside the water block is a descriptor claiming a layout the image is not in, on an image that is an attachment of the same rendering block. Separately, **the rain pass already binds set 2** (`renderer.rs:1871-1879`), so a new binding on it points at an image the graph never touches on `rain_overhang`/`rain_gantry`/`rain_impact` — still `UNDEFINED`. Whether the layers fire depends on static-use analysis, which `cmaa2.rs:149-154` says *"is not a thing to be clever about"* — and both plans quote that line and then do it.
**The objection blocking a fourth set is factually wrong.** `renderer.rs:2874-2877`: *"a second one would mean a second push-constant range to keep in step."* `VkPipelineLayoutCreateInfo` takes them independently — verified at `renderer.rs:2884-2886`, `.push_constant_ranges(&ranges).set_layouts(&sets)`. `maxBoundDescriptorSets = 32` on this device (`vulkaninfo`). **Set 3, its own 60-line module. `scene_depth.rs` is not touched, not renamed, not extended.**

**6. `ms_depth` is stored every frame for nothing — 33 MB/frame at 1080p, free to reclaim, and it is the same third-state problem as colour.**
`renderer.rs:2336-2341` sets depth `store_op = STORE` unconditionally, with a comment about the rain pass. When multisampled, the attachment is `ms_depth` and rain samples the *resolved* `depth` (`renderer.rs:1839`, `Access::DepthSample`; the rain block has no depth attachment at all, `begin_overlay_rendering`, `renderer.rs:3337`). Nothing reads `ms_depth`. The comment is attached to the wrong image. Take the baseline measurement with this fixed, or the refraction delta is partly cancelled by a bug fix.

**7. The nominal bandwidth tables in both plans are falsified by this repo's own numbers.**
ADR `0013-the-water-mesh-reaches-the-horizon.md:144`: *"The forward pass on `ocean` at 1920×1080 was 0.061 ms."* That pass already contains two resolves = 82.94 MB nominal = **0.082 ms** at the 4090's 1008 GB/s. Measured is below the theoretical floor of its own resolves, so nominal traffic overstates by ≥35% (MSAA colour compression). And ADR `0014-rain-drops-stay-stateless-for-now.md:24-29`: the rain pass — LOAD a multisampled pair, draw 160,000 streaks, AVERAGE-resolve, at 1080p/4× — costs **0.036 ms**. That *is* the manoeuvre both plans propose. Plan 2's "≈108 MB ≈ 0.11 ms" would have argued someone out of a ~0.02 ms change.

**8. Scene geometry, verified — three claims in the plans are wrong.**
Box meshes scale about their centre.

| scene | object | span | still level | state |
|---|---|---|---|---|
| `ocean` | PostNear `:156` `pos.y 0.8, scale.y 3.2` | −0.80…2.40 | 0.0 `:75` | crosses, 0.8 m submerged |
| `ocean` | PostFar `:168`, Rock `:182` | −0.80…2.00 / −0.90…0.20 | 0.0 | crosses |
| `shore` | Post `:148` `pos.y 6.6, scale.y 2.6` | 5.30…7.90 | 8.0 `:102` | **fully submerged, top 0.1 m under** |
| `river` | Post `:186` `pos.y 7.4, scale.y 1.6` | 6.60…8.20 | 6.5 | **never crosses; entirely above water** |
| `homestead` | MooringPost `:567` | 6.75…8.65 | 8.0 | crosses — **the only golden scene that does** |

So Plan 1's slice-3 artifact analysis is anchored on `river`, which cannot exhibit foreground bleed at all, and both plans' "must be byte-identical" control set is wrong: **`ocean` will move in slice 2** — its three submerged objects sit at 0.3–1.5 m of path, `T_red = 0.6–0.9`, and they *should* appear. The true zero-delta controls are **`squall`** (sea and sky, no geometry) and **`underwater`** (`eyeUnderwater()` returns at `scene.slang:2670`).

**9. `puddles` has no `WaterBody`.** `puddles.loom:16,18` are prose saying a puddle is deliberately not one. Six golden water scenes, not seven. The brief is wrong; both plans caught it.

**10. Adding a scene to `GOLDEN` silently enrols it in `shimmer` and `flythrough`.** `xtask/src/main.rs:517` and `:684` both iterate `GOLDEN`. Neither plan said so, and this project's scar is a shimmer metric that framed a field containing no grass.

**Criticisms rejected, one line each.**
— *Measure a one-depth-resolve variant before buying the second depth image* (Critique 2 §10): rejected as a gate, kept as a note — it forks pass topology mid-plan to save ~0.01 ms by ADR 0014's own evidence; conservative reading wins.
— *Split set 2 vs. keep one set* (critics disagree): took the conservative reading, a separate set 3.
— *`shore`'s post is a foreground-bleed test* (both plans): rejected, it is fully submerged; `homestead` is the bleed scene.
— *Judge the offset sign on a 320×200 golden* (both plans): rejected, a 0.12 m rod is ~4 px there; judged at 1280×800, the golden is a hash tripwire.

---

## Verdict

A second rendering block: the opaque half of the forward pass resolves colour and depth into two dedicated single-sample images, and water — still at 4× MSAA, in the same multisampled pair, loaded — samples them, so the through-water term becomes the actual seabed attenuated over the actual ray length instead of a two-endpoint tint. That is Unreal's Single Layer Water topology, and Loom gets `SceneDepthWithoutWater` free because the opaque half was resolving depth anyway.

**Honest gap to Unreal once built:** no per-water-body optical parameters (two `static const` in a shader, so `river` silt and open ocean cannot differ — this is the largest remaining gap and the one that most directly answers "polished pewter"); one depth layer, so no back faces and no fix for the featureless mirror the underwater branch already admits at `scene.slang:2661-2666`; no rough refraction (`WATER_ROUGHNESS = 0.08`, not needed); no downsampled refraction option; refraction of the offset lookup is a single plane intersection, not a march.

---

## The plan

### Slice 0 — turn the turbidity knob in a temp dir. Commit nothing.

**What.** In `$(mktemp -d)`, copy the tree, raise `WATER_BACKSCATTER` (`scene.slang:2508`) 10× and 30×, `cargo xtask image`, open `ocean.png` and `squall.png`.

**Why this order.** `WATER_DEEP` is derived from the backscatter of *distilled* water and its own comment calls it *"the turbidity knob, and the only one"*. `ocean` and `squall` have no `VoxelVolume`; under any correct formulation `T = 0` there and **refraction cannot change them by one pixel**. They are also the two most grazing cameras (−7°, −3°) — i.e. exactly the "polished pewter under overcast" case the human measured. If the knob answers it, the rest of this plan is aimed at the wrong pixel.

**Files / shader / passes / gates / cost.** None. Zero committed diff.

**Test and mutation.** Not applicable — this is a look, not a build. **Abort condition:** the knob alone fixes `ocean`. **Proceed condition:** `shore`/`river` still read as pewter *over a seabed at a steep angle*.

**Risk.** That it gets skipped because it is not a commit. It is the cheapest rung on the ladder and the grass-density-falloff lesson applied preventively.

---

### Slice 1 — split the pass. Twenty-one references byte-identical.

**What.** Two new single-sample images per render path; the forward pass draws sky/meshes/grass and resolves into them; a new `water` pass loads the multisampled pair, draws water then particles, and carries the final resolves. **No shader change at all.**

**Why this order.** It is the only commit where a render-graph topology change is isolated from a shading change. When the optics land wrong in slice 2 — and §Part 0 items 1–4 say they will if anything is skipped — bisection has a clean boundary. Also fixes the dead `ms_depth` STORE so slice 2's timing delta is measured against a clean baseline.

**Files.**

| file | change | ~lines |
|---|---|---|
| `crates/loom_render/src/renderer.rs` | two `create_image` calls beside `:730-773` — `loom.scene_opaque` (`COLOR_ATTACHMENT\|SAMPLED`) and `loom.depth_opaque` (`DEPTH_STENCIL_ATTACHMENT\|SAMPLED`), both `TYPE_1`; four fields; teardown at `:2170-2190`; `Resolve` gains `keep_samples: bool` and `begin_rendering` a `load: bool`; store-op rules at `:2323-2327` **and `:2341`**; split at `:1626`/`:1762`; drop `TRANSIENT_ATTACHMENT` from `msaa_depth` (`:411-412`) | ~200 |
| `crates/loom_render/src/viewer.rs` | the same two images in `Viewer::new` **unconditionally, outside the `aa` option** (with `LOOM_CMAA2=0` the scene target *is* the swapchain image, `viewer.rs:1128`, usage `COLOR_ATTACHMENT` alone, `viewer.rs:1906`); **rebuilt in `recreate` at `:1616-1693`**; same split at `:1161`/`:1272` | ~180 |
| `crates/loom_render/src/water_textures.rs` | new. Set 3: binding 0 `opaqueColor` (**LINEAR**, CLAMP_TO_EDGE), binding 1 `opaqueDepth` (**NEAREST**, CLAMP_TO_EDGE — `scene_depth.rs:13-15` gives the reason and it still applies). One pool, one set, written at startup and on resize. `scene_depth.rs` untouched | ~70 |
| `crates/loom_render/src/renderer.rs` (layout) | `sets` at `:2879` gains a fourth entry; fix the wrong comment at `:2874-2877` | ~6 |
| `crates/loom_render/src/lib.rs` | one new test | ~45 |
| `xtask/src/main.rs` | `water_crate` and `splash` into `GOLDEN`; fix the stale comments at `:69-72` and `:837-846` and at `renderer.rs:3023-3026`, `:3102-3105`, `:3205` | ~25 |

**Shader.** Two `[[vk::binding(n, 3)]]` declarations only, unread. Declared here so slice 2 is pure shading; an unread but *bound* descriptor is legal.

**Passes and `Access` declarations.**

```rust
let split = water_verts > 0 && msaa_ids.is_some();   // water_verts: renderer.rs:1582
let rain_resolves = msaa_ids.is_some() && rain_buffers.is_some();   // unchanged, :1615
```

| pass | accesses |
|---|---|
| `forward` (split) | `(ms_color, ColorWrite)`, `(ms_depth, DepthWrite)`, `(scene_opaque, ColorWrite)`, `(depth_opaque, DepthResolve)` — `keep_samples: true` |
| `forward` (no split) | **unchanged, character for character** |
| `water` (new) | `(scene_opaque, ShaderRead)`, `(depth_opaque, DepthSample)`, `(ms_color, ColorWrite)`, `(ms_depth, DepthWrite)`, `(depth, DepthResolve)`, and `(color, ColorWrite)` unless `rain_resolves` |
| `rain` | **unchanged** (`renderer.rs:1838-1842`) |

No new `Access` variant: `ShaderRead` on a colour image is `cmaa2_edges` (`renderer.rs:1926`); `DepthSample` is rain (`:1839`, and its `DEPTH` aspect mask at `loom_render_graph/src/lib.rs:112-118` is why it is not `ShaderRead`). No new pipeline — water and grass share `create_geometry_pipeline` and the second block has the same format, `DEPTH_FORMAT` and sample count. No push-constant change (`Push` is at 124/128, asserted `lib.rs:545-552`). No `EnvironmentData` change — `viewport` is at `scene.slang:88` and `invViewProj` at `:161`, both already in the push block.

**Three mechanical points, each with a named failure.**
- **`keep_samples` covers colour *and* depth.** Colour today is `DONT_CARE` iff resolving (`:2323-2327`), which is exactly wrong for the opaque half — water composites onto undefined samples. Depth today is `STORE` unconditionally (`:2341`), 33 MB/frame for nothing; make it `if depth_resolve.is_some() && !keep_samples { DONT_CARE } else { STORE }`. One flag, both attachments, `Default` so every existing call site is unchanged.
- **The depth resolve into `loom.depth_target` moves to the `water` pass**, so rain still sees post-water depth exactly as today. Leave it in the opaque half and rain streaks pass through the sea on `squall`/`homestead` — no validation message, no barrier-list change, no other scene affected.
- **Never declare one image twice in one pass.** `plan_full`/`execute` iterate accesses in order calling `decide` once each (`loom_render_graph/src/lib.rs:602-607`, `:672-677`): two accesses emit two back-to-back barriers and leave the last layout. It does not error. This is why `depth_opaque` exists as a second image rather than resolving twice into one.

**Scenes and gates.** `SCENES` unchanged. `GOLDEN` gains `water_crate` (`--sim 90`, the only scene with a buoyant body through the surface) and `splash` (`--sim 120`, particles at the waterline — precisely what this slice moves). **Bless both on the unsplit tree first, then apply the split, then require 23/23 unchanged.** A reference blessed after the split proves nothing.

**Test.** New, beside the existing barrier test (`Renderer::environment` is public, `renderer.rs:605`):

```rust
#[test]
fn the_water_pass_reads_what_the_opaque_pass_left() {
    // …renderer.environment.water = [.., .., 1.0, ..]; one wave…
    assert_eq!(&transitions[..6], [
        ("forward", "loom.msaa_color"), ("forward", "loom.msaa_depth"),
        ("forward", "loom.scene_opaque"), ("forward", "loom.depth_opaque"),
        ("water",   "loom.scene_opaque"), ("water",   "loom.depth_opaque"),
    ]);
}
```

**Mutation that must break it.** Delete `(depth_opaque, Access::DepthSample)` from the water pass. The image is never moved to `SHADER_READ_ONLY_OPTIMAL`, the sixth entry vanishes, the test fails — *and* `cargo xtask validate` reports a layout error on all eight water scenes. Two independent gates. Second mutation, by hand: `keep_samples: false` on the opaque half produces no validation message and no test failure and shows up only as garbage water in slice 2 — which is why `water_crate` and `splash` join `GOLDEN` **here**.

**Existing barrier test at `crates/loom_render/src/lib.rs:382-420` does not change by one character.** Its scene is two primitive meshes and never sets `environment.water[2]` (`:335`), so `water_verts == 0` and `split` is false. **That is a design constraint on the change, not a happy accident** — it is what keeps the fifteen non-water references and every non-water frame at literally zero cost.

**Cost.** At 1920×1080, 2,073,600 px, RGBA8/D32: 1× = **8.294 MB**, 4× = **33.18 MB**; one resolve = 33.18 R + 8.29 W = **41.47 MB**.

| item | nominal |
|---|---|
| second colour resolve | +41.47 |
| second depth resolve | +41.47 |
| `ms_color` DONT_CARE→STORE in opaque half | ≤ +33.18 (≈0 on an IMR — samples are already resident) |
| `ms_depth` STORE deleted on non-split frames | **−33.18** |
| new VRAM | 16.6 MB per path (0.07% of 24,564 MiB) |

Nominal delta ≈ **+83 MB**. **Prediction, falsifiable:** `ocean` at 1920×1080 is 0.061 ms today with two resolves in it (ADR 0013:144); the rain pass — LOAD + 160k streaks + resolve — is 0.036 ms (ADR 0014:24-29). `forward` + `water` together should land **0.07–0.09 ms**. Above 0.12 ms, something other than the resolve is wrong.

**Risk.** The viewer's `recreate` (`viewer.rs:1616-1693`) — an image not rebuilt, or a descriptor not repointed as `self.scene_depth.bind(depth_view)` is at `:1633`, is a dangling view under a live descriptor with **no layer symptom** once the handle is reused (the code says so at `:1630-1633`). No headless gate sees it; the first window resize does.

---

### Slice 2 — the seabed appears. No distortion.

**What.** Path length from the depth buffer, two-leg absorption, fog unwind, and the deletion of the tint lerp. This is the whole feature: at a steep view `fresnel = 0.02` (`scene.slang:2676`) and the through-water term is 98% of the answer.

**Why this order.** Largest image change per line in the plan, and no offset means no foreground bleed, no waterline halo, no screen-edge case, and nothing that could tempt a derivative.

**Files.** `assets/shaders/scene.slang` (~35 net, a net deletion of two constants), `assets/test/refraction.loom` (new), `xtask/src/main.rs` (three lines).

**Shader.** One helper, placed before `waterFragmentMain` at `:2603`. It is `rainDepthFade`'s body (`:2951-2958`) with the subtraction kept:

```slang
/// Effective secant of the downwelling path. **Not the sun's.** Light reaching
/// a seabed is sun *plus* sky and the sky arrives near-vertical, so the true
/// solar secant (2.9 on `shore`) would attenuate the whole hemisphere as one
/// grazing beam. Calibrated against `shore`.
// ponytail: one scalar for the whole hemisphere. Split the sun and sky legs
// only if a low-sun scene reads wrong.
static const float WATER_DOWN_SEC = 1.5;

/// What is behind the surface, un-fogged, with the two-leg transmittance.
///
/// **`SampleLevel` only.** The shoreline `discard` is the first statement of
/// `waterFragmentMain` (:2614); whether the compiler emits `OpKill` or
/// `OpDemoteToHelperInvocation` decides whether a killed lane contributes to a
/// quad derivative, and the affected quads are exactly the waterline `shore`
/// and `river` are golden to protect.
float3 waterBehind(float2 uv, float3 surfaceWorld, float3 fogDir, out float3 T) {
    float d = opaqueDepth.SampleLevel(uv, 0.0).r;
    // **Nothing behind: sky, the sentinel, and the 50 km skirt past a 1000 m
    // far plane (:2478), all at once.** Guarding on the raw sample rather than
    // on the reconstructed distance is what makes the skirt opaque instead of
    // a transparent hole, and it is why `squall` comes out byte-identical.
    T = float3(0.0);
    if (d >= 1.0) { return float3(0.0); }
    float3 eye = push.environment[0].eye.xyz;
    float4 u = mul(push.invViewProj, float4(uv * 2.0 - 1.0, d, 1.0));
    float3 bg = u.xyz / u.w;
    // Two legs. `behind` was shaded in air; the light that reached it crossed
    // the column downward first. Lee et al.'s shallow-water form.
    float up   = max(distance(eye, bg) - distance(eye, surfaceWorld), 0.0);
    float down = max(surfaceWorld.y - bg.y, 0.0) * WATER_DOWN_SEC;
    T = exp(-WATER_EXTINCT * (up + down));
    // The mesh pass already fogged this pixel over eye->bg (:1621) and the
    // water's own fog covers eye->surface again; over `shore`'s 50 m that turns
    // 0.18 into 0.33. Clamped at 0.15, past which the surface is fogged out too.
    float fb = fogAmount(eye, bg);
    return max((opaqueColor.SampleLevel(uv, 0.0).rgb - fogColor(fogDir) * fb)
               / max(1.0 - fb, 0.15), 0.0);
}
```

Replacing `:2713-2716`, and **deleting `:2678` with `WATER_SHALLOW` (`:2530`) and `WATER_TINT_DEPTH` (`:2532`)**:

```slang
    float3 T;
    float3 behind = waterBehind(in.clip.xy / push.environment[0].viewport.xy,
                                in.worldPos, -view, T);
    float3 downwelling = hemisphereAmbient(normal) * ambientStrength() * skyView
                       + sunColor() * sunStrength() * toSun;
    float3 through = behind * T + WATER_DEEP * downwelling * (1.0 - T) + sss;
```

`in.depth`'s only remaining consumer is the discard. `body` is gone.

**Passes and `Access`.** None new. Bind set 3 at the top of the **water block only**.

**Scenes and gates.** `assets/test/refraction.loom` — new, into `SCENES`, `GOLDEN` (`--sim 90`) and the **windowed list** (`xtask/src/main.rs:847-874`) beside `ocean` and `shore`. The six existing water scenes cannot protect this: every downward camera is −3° to −14°, so `fresnel` is still 0.27 at frame centre on the steepest; and nothing behind any water surface in this project carries a pattern (sand `[0.52,0.45,0.34]`, silt `[0.38,0.34,0.27]`).
- Camera `pos = [16.0, 9.5, 26.0]`, `rot_euler = [-42, 0, 0]`, `fov_y_degrees = 55` — 3.5 m up at 42° depression, `fresnel` 0.023–0.09 across the whole band.
- `VoxelVolume`, `voxel_size = 0.25`, `chunks = [4,2,4]`; bed at `y = 3.0` plus a shelf so depth ramps **0.2 m near → 4.0 m far**, surface at `y = 6.0`. The ramp makes `exp(-c·path)` legible as a gradient and puts the shoreline `discard` inside the frame.
- **`albedo_map` with `FLAG_TRIPLANAR`** on the bed — the path `ground.loom` exercises. Without a pattern, slice 3's wrong-sign case renders identically to the right-sign case.
- A **0.4 m rod at 55° from vertical** crossing the surface near the camera (~10 px of submerged displacement at 1280×800), a submerged sphere ~1 m under, and a box on the shelf so the waterline crosses a straight edge where `path → 0`.
- One 3 m ripple, amplitude 0.05. The subject is the bed.

**Test.** `refraction` in `GOLDEN` with the measured stub percentage in its comment, per the `puddles` 0.8% / `rain_overhang` 18.5% / `ground` 63.6% precedent.

**Mutation.** Replace the through-water expression with `WATER_DEEP * downwelling` (stub refraction out), render, `loom compare`. **Expect >20%. Under 2%, fix the scene, do not accept the number** — `rain_impact`'s 0.016% is described at `xtask:239-247` as protecting nothing. Second mutation: delete the `d >= 1.0` guard → `squall` moves. Third: drop the `down` leg → `shore`'s shallows go bright and neutral; visible against the reference.

**Goldens moved:** `shore` (largest — ~60% water over a 0–6 m sand bed), `homestead`, `river`, `ocean` (its three submerged objects appear — **this is the feature working**, not the reconstruction failing), `water_crate`, `refraction`.
**Byte-identical controls:** `squall` (sea and sky, no geometry) and `underwater` (`eyeUnderwater()` returns at `:2670`). **Put in the commit message that `ocean` moving is expected and `squall` not moving is the check** — both plans had this backwards.

**Cost.** Two `SampleLevel` + one unproject + `exp3` + two `distance` per water fragment, over water fragments only, **skipped entirely wherever the background is sky** because the guard returns first — on `ocean` and `squall` the whole wavefront takes the same side, so the two scenes that gain nothing pay nothing. Texture traffic ≤ 2 × 8.29 MB at 1080p, cache-resident (1:1 lookup). Note `discard` at `:2614` already disables early-Z for this pipeline, so on `ocean` every pixel below the horizon shades — measure, do not infer from "0.061 ms".

**Risk.** `skyView` (`:2687`) accidentally scaling `behind` — it must multiply `downwelling` only; the seabed was lit by its own pass and darkening it 0.55 in a trough is a reason that has nothing to do with the seabed. Small, plausible, and a `--bless` would swallow it.

---

### Slice 3 — the offset.

**What.** Bend the ray at the real IOR, project where it lands, fade at the frame edge, attenuate the offset near foreground geometry.

**Why this order.** Its only effect in the six existing scenes is artifact — their beds are flat colours and a distorted flat colour is the same colour. Ship it only after opening `refraction.png` and confirming slice 2 left a visible want.

**Files.** `assets/shaders/scene.slang` (~20).

**Shader.** Replacing the `uv` argument to `waterBehind`:

```slang
    // **No strength knob.** The offset weakens with distance for free, because
    // the same world displacement subtends fewer pixels further away — which is
    // what `normal.xz * strength` cannot do. No TIR guard, and the underwater
    // branch's (:2653) must not be copied: air->water gives sinT2 <= 0.5628 at
    // every incidence angle, so it would advertise a case that cannot occur.
    float3 bent = refract(-view, normal, 1.0 / WATER_IOR);   // eta = 0.7502
    float4 hit  = mul(push.objects[push.objectOffset].mvp,
                      float4(in.worldPos + bent * upPath, 1.0));
    float2 uvR = uv0;
    if (hit.w > 0.0) {
        uvR = (hit.xy / hit.w) * 0.5 + 0.5;
        float2 e = min(uvR, 1.0 - uvR);
        uvR = lerp(uv0, uvR, saturate(min(e.x, e.y) / WATER_REFRACT_EDGE));
    }
    // Attenuate — not reject — where the offset lands on something in front of
    // the water. A hard switch traces the object's silhouette as an outline
    // that breathes with the waves, and SAMPLE_ZERO depth (renderer.rs:2357)
    // gives the test no coverage information to soften it with. The un-offset
    // UV can never be wrong: this fragment *is* water there.
    uvR = lerp(uv0, uvR, smoothstep(0.0, WATER_REFRACT_ACCEPT, pathAt(uvR)));
```

`push.objects[push.objectOffset].mvp` is the reserved slot's view-projection (`renderer.rs:503-509`), already used by `waterVertexMain` at `:2471`. `view` is surface→eye (`:2619`), `normal` is flipped eye-ward (`:2624-2626`), and `:2659` calls `refract` with those exact shapes for water→air passing `WATER_IOR`, which pins the convention: `eta = n_incident/n_transmitted`, so air→water is the reciprocal. **The absorption path stays the un-offset one** — it is the column under this fragment and must not flicker with the offset. **The waterline needs no separate term**: `path → 0` there because the background *is* the beach the water lies on.

**Scenes and gates.** No new scene. `refraction.loom`'s rod is the test.

**Mutation.** Three, each a distinct picture at 1280×800: sign flipped (`- bent`) → the submerged half bends *away* from vertical; `smoothstep` arguments swapped → the rod's *above*-water half smears sideways into the water; `behind` forced to `WATER_DEEP` → flat tint, no bend, no bed pattern. **All three are numerically plausible, so clippy, `cargo test` and `cargo xtask validate` pass every one** — verified: `the_water_draw_matches_the_shader_s_grid` (`lib.rs:172-218`) reads only `WATER_RES`/`WATER_LEVELS`, water shading is outside the sim hash, and a wrong sign produces no validation message. **The 320×200 golden is a hash tripwire; correctness is judged at 1280×800.** Any plan that does not name the resolution is hiding that its gate protects nothing.

**Cost.** One extra `SampleLevel` (depth at the offset UV), one `refract`, one mat-vec, two `smoothstep`. No graph, image or descriptor change.

**Goldens moved:** `refraction`, `homestead` (the only golden scene with crossing geometry), `water_crate`, `ocean` slightly. `squall`/`underwater` still byte-identical; `shore`/`river` should barely move — their beds are flat colours, so a large move means the offset is far too big.

**Risk.** `river`'s 1.9 m ripples (`river.loom:147`) at ~3 m range under a −14° camera are where an over-strong offset reads as boiling. **Judge strength there, not on `ocean`.**

---

## The MSAA decision

**Water stays at 4×, in a second rendering block that `LOAD`s the pair.** `MSAA_SAMPLES = TYPE_4` (`renderer.rs:336`) is untouched, the pipelines are unchanged, the split is gated on `msaa_ids.is_some()`.

The arithmetic: at 1080p a 4× RGBA8 image is 33.18 MB and a resolve is 33.18 R + 8.29 W = **41.47 MB**. The split pays two of those (colour and depth) plus a `STORE` on `ms_color` that an immediate-mode GPU largely does not, and reclaims a 33.18 MB `ms_depth` `STORE` that is currently wasted (`renderer.rs:2341`). Nominal +83 MB; the measured comparison is that `ocean`'s current forward pass contains two resolves (82.94 MB nominal, 0.082 ms at 1008 GB/s) and runs in **0.061 ms** (ADR 0013:144), and the rain pass — the same LOAD/draw/resolve manoeuvre with 160,000 streaks in it — costs **0.036 ms** (ADR 0014:24-29). So the structure costs roughly 0.01–0.02 ms.

Water is **not** single-sampled, for four reasons: particles depth-test (`renderer.rs:3134-3136`) and draw after water (`:1786`), and `splash` spawns them at the waterline; rain already carries the colour resolve, so water after it draws *in front of* the rain on `squall`/`homestead`; single-sampled water would depth-test against `loom.depth_target`, a `SAMPLE_ZERO` resolve (`:2357`) that stair-steps the waterline `shore` and `river` are golden to protect; and `WATER_ROUGHNESS = 0.08` is documented (`scene.slang:2536`) as the smallest roughness that reads as glitter rather than a hard dot — the sub-pixel-specular case where ADR 0010 measures MSAA at −30% and CMAA2 at −8.8%. There is **no 1× row for any water scene anywhere in `docs/`**; if it is ever reconsidered, that is the missing run.

**What it costs the horizon: nothing.** The horizon is an ~8/255 step against fogged sky (ADR 0013:18-22) and it resolves at 4× exactly as today. The split does not touch the rasterisation of a single water fragment; it only changes what the block reads and when it resolves.

**One consequence to write down rather than fix:** no sample shading is enabled anywhere (`grep sample_shading` over `crates/loom_render/src/*.rs` is empty), so all four samples of a water pixel share one pixel-centre refraction fetch. Water's own silhouettes keep 4×; the bed seen through it carries whatever survived the resolve. Unreal does the same.

---

## The blend algebra

```
T           = exp(−WATER_EXTINCT · (L_view + H_bed · WATER_DOWN_SEC))       [0..1]³
behind      = un-fogged opaque colour at the (offset) UV                     lit in air
downwelling = hemisphereAmbient(n)·ambientStrength()·skyView
            + sunColor()·sunStrength()·max(dot(n, sunDir), 0)
sss         = sunColor()·sunStrength()·pow(dot(−view, h), P)·crest·exp(−c·thick)

through = behind·T  +  WATER_DEEP·downwelling·(1 − T)  +  sss
lit     = lerp(through, skyColor(reflect(−view, n)), fresnel)
lit    += specular(n, view, sunDir, WATER_ROUGHNESS, 0.02)·sunColor()·sunStrength()·toSun
lit     = lerp(lit, foamLit, foam)
out     = lerp(lit, fogColor(−view), fogAmount(eye, worldPos))
```

**`through` is not double-counted.** `through` at `:2713-2716` *already is* the "seeing into the water" term, and `body = lerp(WATER_SHALLOW, WATER_DEEP, in.depth/WATER_TINT_DEPTH)` at `:2678` exists solely to fake the bottom showing through. Both are **deleted**, not supplemented. The new expression is the single-scatter decomposition of which the old one was an endpoint: `L_view → ∞` gives exactly `WATER_DEEP·downwelling`, today's answer; `L_view → 0` gives the background unattenuated, the shoreline. `WATER_SHALLOW` was a hand-picked *linear* stand-in for the middle with no view-angle dependence. The `T` / `(1−T)` partition conserves energy by construction — this is why adding a background does not brighten the water.

**`WATER_BACKSCATTER` is not in the exponent.** `WATER_EXTINCT` is *already* the beam attenuation `c` (Pope & Fry absorption plus Morel scattering, `:2501-2503`); `b_b` appears only in the derivation of `R∞ = WATER_DEEP` at `:2528-2529`. Adding them double-counts scattering a second, subtler way.

**`sss` is not double-counted.** Different transport path: sun entering the *far* side of a crest and emerging toward the eye. Gated on `crest` and on its own `thick` (`:2706-2707`), view-dependent through `dot(−view, h)`, describing light that never travelled the view ray through the column. It stays additive.

**`skyView` scales the volume term only.** `behind` was lit by its own shading pass; multiplying it by 0.55 in a trough would darken the seabed for a reason that has nothing to do with the seabed.

**Fresnel is untouched, and it is the whole answer to the human's finding.** `lerp(through, sky, fresnel)` is the energy split and both branches are now individually correct: grazing → `fresnel → 1`, refraction correctly suppressed to nothing (the polished pewter is *partly right physics*); steep → `fresnel = 0.02`, and the 98% that was a near-black constant is now the seabed.

**Foam needs no suppression term, only a placement rule.** `lit = lerp(lit, foamLit, foam)` at `:2755` is a material swap (`:2751-2754`) applied *after* the Fresnel lerp that consumes `through`. Fold refraction into `through` before `:2718` and you cannot see the seabed through a whitecap. Add it after and a breaking crest becomes a window onto the sand.

**Fog is applied once per segment.** `behind` is un-fogged by `waterBehind` over eye→bg; the final line re-fogs the composite over eye→surface. Residual error is the surface→bg segment (1–2 m), under a percent.

---

## The measurement

**Instrument 1 — the image gate, and the number that makes it a gate.** Stub refraction to `WATER_DEEP * downwelling`, render `refraction.loom` at `GOLDEN_SIZE` (`xtask/src/main.rs:313`), `loom compare`. **Target >20%**, against the calibrated 0.1% / 64 px threshold and this project's own precedents (`puddles` 0.8%, `rain_overhang` 18.5%, `ground` 63.6%). **How the subject is guaranteed to be in the frame:** the scene's camera is authored at a 42° depression over a bed that ramps 0.2→4.0 m *inside the frame*, so water is not merely present, it is present at every path length the term is defined over — and the acceptance rule is *if the stub diff comes in under 2%, fix the scene, not the number*. That rule is the whole defence against the grass-density-falloff failure, where a metric rewarded whatever deleted the subject.

**Instrument 2 — GPU timestamps, at a resolution where the change exists.** `LOOM_GPU_TIMING=1 loom render assets/test/ocean.loom --size 1920x1080 --sim 90`, medians of eight frames, ADR 0012's protocol. **Not the gates** — `image` renders at 320×200 and `shimmer` at 640×400, where the extra resolve is 1.28 MB ≈ 1.3 µs and below the noise floor of a timestamp pair; a plan that "measures with `LOOM_GPU_TIMING=1`" and runs the gates measures nothing. Read the `forward` and `water` rows, **never the `graph` total** — it includes the PCIe-bound readback (13.5–14.0 GB/s), which at 1080p dwarfs everything. Baseline 0.061 ms (ADR 0013:144), prediction 0.07–0.09 ms combined, fail above 0.12 ms. `blockout` is the no-water control and must be unchanged to the digit.

**Instrument 3 — flicker, used only where it means something.** `cargo xtask shimmer` baselines at 4× are `ocean` 1.945, `shore` 2.016, `underwater` 2.597, `river` 0.566 (ADR 0013:353-358). The `shore`/`river` delta is **uninterpretable** — a seabed seen through a moving surface legitimately moves, and CLAUDE.md's rule against comparing AA numbers across a change in colour applies with force. Two legitimate uses: `squall` and `underwater` as zero-delta controls (more than a couple of percent means something is refracting that should not be), and two *strengths of one formulation on one scene* in slice 3. **Adding `refraction` to `GOLDEN` auto-enrols it in shimmer and flythrough** (`xtask/src/main.rs:517`, `:684`), which frame whole-scene bounds rather than the authored camera — so **before quoting any number from it, open `target/xtask-shimmer/refraction_0000.png` and confirm the seabed is in the frame.**

**Instrument 4 — the one for popping, because nothing else can see it.** `shimmer` bolts the camera down and `flythrough` pans, and a pan is chosen precisely because it makes no parallax. Disocclusion — the offset attenuation band sweeping a silhouette — is judged on `loom render --dolly` (`crates/loom_cli/src/main.rs:575`, `:817`) toward the beach on `shore` and across the channel on `homestead`, at 1280×800.

---

## ADRs needed

**ADR 0018 — Water refraction splits the forward pass.** Next free number (`docs/decisions/` runs to `0017-raindrops-become-stateful.md`). One decision: **the forward pass is split at the water draw so that water can read a resolved copy of what is behind it, and the split is gated on there being water.** The body records: the four reasons water keeps 4× MSAA; the third store-op state covering colour *and* depth, and the dead `ms_depth` STORE it fixes; the second depth resolve and why one image will not do (rain needs post-water depth, water needs pre-water); the deletion of `WATER_SHALLOW` and `WATER_TINT_DEPTH` with the `path → ∞` limit argument that makes it safe; the two-leg transmittance and `WATER_DOWN_SEC`'s ceiling; the `d >= 1.0` guard and the 1000 m far plane against a 50 km skirt; and the contrast with **ADR 0017** — state was the price rain paid, refraction pays none, the source is the previous draw of the same frame, so a frame stays a pure function of its tick and the sim hash `b478ea4ac2622d32` is untouched. It must cite and distinguish **ADR 0010** (no post-process before Phase 8, moved once for CMAA2): refraction is **not** a post-process — it is inline shading of a surface, in the geometry pass, at the geometry's sample count, adding no full-screen pass. And it updates **ADR 0011** in one line: `in.depth` survives, but only as the shoreline test; it is no longer an optical quantity.

No other ADR. Everything else is inside locked decisions: dynamic rendering only (no subpass, no input attachment; `VK_KHR_dynamic_rendering_local_read` is supported on this driver and is explicitly rejected — it guarantees a read only at the current fragment's location and refraction reads neighbours by construction), `gpu-allocator` only, every barrier in the graph, no per-draw descriptor set, no trait with one implementation, all resources named through `DebugNames`.

---

## What we are not doing

- **Per-water-body optical parameters.** The single largest remaining gap to Unreal and the thing most likely to answer "polished pewter" — but it is a `WaterBody` schema change, an `EnvironmentData` field and a scene-format decision. **Changes it:** slice 0 showing that the turbidity knob is what the human wants, or `river` and open ocean needing to differ.
- **A second full depth image for slice 4-style true rejection.** Not needed — the depth we already added serves both the path length and the offset attenuation.
- **Rough/frosted refraction, a colour mip pyramid.** `WATER_ROUGHNESS = 0.08`; the surface is smooth. Godot's own tracker (#108935, #88786) records mipmapped screen textures costing milliseconds and artifacting. **Changes it:** a `WaterBody` that authors a rough surface.
- **A half-resolution refraction target.** Unreal offers it as a quality *tradeoff*. Neither `vkCmdResolveImage` nor a resolve attachment can scale, so it costs resolve-to-full then blit — an extra pass to save ~6 MB against a 0.061 ms forward pass. **Changes it:** a measured pass over 0.3 ms.
- **Fixing the underwater branch's featureless mirror** (`scene.slang:2661-2666`). It needs a screen-space *reflection* march; the reflected direction points steeply away from the view ray. `underwater.png` staying byte-identical is the scoping proof.
- **Sampling the previous frame's resolved colour target.** Zero passes, zero images — and dead twice: a golden render is one frame, so "previous" is the clear colour, and with `LOOM_CMAA2=0` the viewer has no readable scene image (`viewer.rs:1128`, `:1906`). Written down because someone will propose it at 3am.
- **`Texture2DMS` with a manual 4-tap resolve, a `vkCmdCopyImage` scratch, a second opaque render.** Each needs the split anyway and then costs more than the resolve it replaces.
- **Water at one sample.** Four reasons above. **Changes it:** a `shimmer` run at `TYPE_1` on `ocean`/`shore`/`river`/`underwater` against 1.945 / 2.016 / 0.566 / 2.597, with `LOOM_CMAA2` both ways.
- **Chasing the `rain_impact` gap** — it is in `GOLDEN:252` and not in `SCENES`, so it has no validation-layer coverage at all. Pre-existing, one-line issue, out of scope.

---

## The single biggest risk

**The optics land wrong in a way that looks deliberate, and `--bless` writes it into `tests/references/` as the new truth.** Three of the four errors in Part 0 — the missing down leg, the double fog, and a `skyView` that accidentally scales the seabed — all push the picture the same direction, toward pale, hazy, over-bright shallows, and all of them produce exactly the large, plausible, "the shallows now show sand" diff on `shore` and `river` that the commit message will claim. Nothing in the four green checks can distinguish a physically wrong seabed from a right one: clippy cannot see a shader, water shading is outside the sim hash, a wrong coefficient emits no validation message, and the image gate proves only that a pixel moved. The only correctness judgement in this pipeline is a human looking at a PNG for two seconds. **The early warning sign is `squall`**: it is sea and sky with no geometry, so under a correct implementation its hash cannot move, and every one of these errors is a formula that touches every water fragment. If `squall.png` moves by one byte, stop — the term is being evaluated where there is nothing behind the water, which means the `d >= 1.0` guard is wrong and every other number in the frame is suspect. The second sign is `shore` at 1280×800: if the far shallows read brighter and *less* blue than the near ones, the down leg is missing; if the whole bed reads washed toward `fogColor`, the unwind is.