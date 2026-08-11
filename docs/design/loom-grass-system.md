<!-- PRECEDENCE NOTE, added on install.

`LOOM-IMPLEMENTATION-ORDER.md` is authoritative on ORDER and supersedes the
"Dependency ordering" list in the Recommendations section below. They agree on
wind and on the golden-image harness. They disagree on one point, and it is
worth stating plainly rather than resolving silently:

  This doc says:  "Procedural scatter system — prerequisite for placement.
                   Land scatter before grass."
  The order says: grass does NOT need the scatter system or prefabs, because a
                  blade is compute-generated geometry derived from a density
                  field and a position hash, not a placed mesh instance. That
                  is precisely why grass sits at Phase 2 and mesh vegetation
                  at Phase 6.

The order wins, and its reasoning holds: what grass needs is the *technique*
(position-hash seeding, Poisson/Halton, dirty-region regeneration), not the
*system*, which exists to place prefab instances into a scene. Requiring the
system would put grass behind S4 prefabs and P5 scatter — month six or later —
and grass is early specifically because it is the forcing function on the
no-TAA decision, which changes the plan for water and rain if it comes out
badly. Discovering that in month eight is the failure this ordering exists to
prevent.

Everything else here — the architecture, the AA verdict, the vertex counts,
the staged plan, and especially the Caveats section — refines Phase 2 rather
than replacing it, exactly as the order document anticipated.
-->

# Realistic, Performant Grass for Loom: A Constraint-Driven Implementation Plan

## TL;DR
- **Build a GPU compute-generated, per-blade Bézier grass system in the Ghost of Tsushima mold** — a placement/cull compute pass that appends surviving blades to a buffer, then `vkCmdDrawIndexedIndirect` renders true geometry blades (15 verts near / 7 far) — but drive placement from a deterministic **position-hash + Poisson/Halton scatter** that mirrors Loom's rain system, keeping blades entirely out of `loom_ecs` and out of the sim hash as purely-visual GPU work.
- **The dominant constraint is "no TAA / no post-process stack."** Almost every shipping AAA grass system (Ghost of Tsushima included) leans on a temporal resolve (TAA/checkerboard/SMAA T2x) to hide sub-pixel shimmer. Loom cannot. The honest verdict: **true opaque geometry blades + 4×/8× MSAA + alpha-to-coverage only where needed + screen-space minimum-width clamping + aggressive distance-based normal-flattening and density management** gets you *most* of the way, but residual specular/edge shimmer in motion is a genuinely unsolved problem without temporal accumulation, and your still-PNG verification will not catch it.
- **Ray-traced shadows for individual blades are disqualified** (millions of dynamic wind-animated blades cannot be economically kept in a BLAS/TLAS); use analytic base-darkening AO plus terrain-level shadow instead. Rough effort for a convincing v1 is ~3–4 weeks for one developer + AI agent; a polished open-world system is ~3–4 months.

---

## Key Findings

1. **The industry-standard architecture is settled.** The talk by Sucker Punch Productions graphics programmer Eric Wohllaib, "Procedural Grass in 'Ghost of Tsushima'" (GDC 2021 Advanced Graphics Summit; GDC Vault play/1027033), established the template every subsequent per-blade system copies: a GPU compute shader generates individual blades, each blade is a **cubic Bézier curve** whose control points come from height/tilt/bend, geometry is generated in-shader from index+instance IDs with **no vertex streams**, blades are **15 vertices at high LOD and 7 at low LOD** (confirmed across the talk and multiple reimplementations), clumping is **Voronoi-based**, and wind is a single unified field sampled by both CPU and GPU. This maps almost perfectly onto Loom's existing design patterns (unified analytic wind, position-hash scatter, bindless buffers).

2. **Loom is unusually well-positioned on three axes and unusually exposed on one.** Well-positioned: (a) it is a *forward* renderer, so MSAA and alpha-to-coverage — the only credible non-temporal AA tools — are actually available (they are broken/unavailable in the deferred renderers that dominate AAA); (b) it already has a deterministic analytic `wind_at(pos,t)` field with a generated Slang version, which is exactly the GoT unified-wind pattern; (c) it has rich terrain analysis (slope/flow/curvature/erosion) that lets placement rules be *computed* rather than hand-painted. Exposed: it has **no golden-image regression testing**, so the exact failure class grass is worst at — motion artifacts (shimmer, popping, swimming) — is invisible to its automated verification.

3. **True geometry blades alias *less* than alpha-tested cards in one dimension and *more* in another.** Geometry blades have no alpha-test edge aliasing (the thing A2C/TAA usually fix) and MSAA anti-aliases their silhouettes directly — a real win for a no-TAA engine. But thin sub-pixel triangles still produce geometric/coverage shimmer as blades cross pixel centers, and specular aliasing on glossy blades is unaffected by MSAA. This is why the recommendation is geometry-first, not cards-first.

4. **Performance is not the hard part on an RTX 4090; overdraw and AA are.** Per the summarized Wohllaib GDC talk, Ghost of Tsushima rendered roughly **83,000 blades on screen at once in about 2.5 ms per frame** on base PS4 hardware, culled from on the order of ~1,000,000 candidates. A 4090 has vastly more headroom. The real budget sinks are (a) MSAA memory bandwidth/cost, (b) overdraw from thin overlapping primitives, and (c) the shadow pass if you naively include grass.

---

## Details

### 1. The compute-generated per-blade pipeline (recommended core)

**Data flow.** Adopt the Ghost of Tsushima structure, adapted for determinism:

- **World divided into tiles.** GoT uses parent tiles carrying a suite of 512×512 textures (height, material, grass-type, clump factor, blade size, wind) at roughly one texel per 39 cm, subdivided into child render tiles, with one indirect draw call per loaded tile. For Loom, the "textures" become **sampled outputs of the terrain-analysis bake** (slope/flow/curvature/erosion) plus the scatter rule outputs — no painted masks, satisfying the text-first constraint.
- **Placement/cull compute shader.** One thread per candidate blade. It computes the blade's world position from a **deterministic scatter sampler** (Bridson Poisson-disk / Halton, seeded from *quantized world position* per Loom's existing scatter design, NOT thread/dispatch order), reads terrain-analysis and clump data, applies **cheap-to-expensive culling in stages** (distance → frustum → orientation/backface → grass-type-empty → zero-height), and appends surviving blades to an append buffer. GoT explicitly orders culling cheapest-first and notes occlusion culling was only a marginal win for grass specifically.
- **Append/consume buffer + indirect args.** Surviving blades go into an append-structured buffer (one per LOD tier is cleanest); the append count is copied into the indirect draw-args buffer to drive `vkCmdDrawIndexedIndirect`. This avoids any CPU roundtrip. In Vulkan with buffer device address (which Loom already has), the blade buffer is addressed bindlessly and the vertex shader reads per-blade data by `instance ID`.
- **Per-blade payload.** GoT's exact shipped struct is unpublished, but the faithful community reconstruction (cainrademan/Unity-Grass) stores: `position` (float3), facing/rotation angle, per-blade `hash`, `height`, `width`, `tilt`, `bend`, surface normal (float3), `color` (float3), `windForce`, `sideBend`, and a clump-color distance-fade. Clump parameters (pull-to-centre, point-in-same-direction, base+random for height/width/tilt/bend) live in a separate small clump table. Keep this payload compact (~48–64 bytes) — it is re-derivable, so you can also store *less* and recompute more in the vertex shader.

**Regenerate every frame vs. persist.** Regenerate the *visible* blade list every frame in compute (it is cheap and it is how GoT works), but derive every per-blade random value from a **position hash** rather than stored state. The right primitive is the PCG hash from Mark Jarzynski & Marc Olano, "Hash Functions for GPU Rendering," *Journal of Computer Graphics Techniques* (JCGT) vol. 9, no. 3, pp. 21–38 (2020). The canonical 32-bit variant is:

```
uint pcg_hash(uint input){
    uint state = input * 747796405u + 2891336453u;
    uint word  = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}
```

with `pcg2d`/`pcg3d` variants for multi-dimensional inputs. This is exactly Loom's rain approach ("stateless GPU particles from PCG hashes outside the sim hash") and is the correct call for grass: it makes partial/dirty-region regeneration over destructible voxels byte-identical because seeds derive from quantized world position, not generation order. **The blade list itself is purely visual and stays out of the sim hash.**

**Geometry authored vs. procedural.** Generate blade geometry procedurally in the vertex shader from the Bézier control points (GoT's no-vertex-stream approach). Do NOT author blade meshes — procedural generation is what enables the LOD vertex-count switching and the "blade folding" trick below.

### 2. Geometry and blade construction

- **Bézier blade.** Represent each blade as a cubic Bézier (4 control points: base `v0`, tip, two mid controls) — or a quadratic (3 control points) if you want to match the simpler Jahrmann & Wimmer "Responsive Real-Time Grass Rendering for General 3D Scenes" (i3D 2017) formulation that most open-source Vulkan implementations use. Control points derive from height/tilt/bend. Vertices are placed along the curve in the vertex shader; **redistribute vertices toward the tip** where curvature concentrates (a GoT trick — even spacing wastes verts on the straight base).
- **Vertex budgets.** 15 verts (high) / 7 verts (low) per blade, matching GoT. That is roughly ~13 triangles high / ~5 triangles low per blade as a triangle strip. A field of ~83k visible blades is on the order of ~0.5–1.5M triangles — trivial for a 4090; the constraint is overdraw and AA, not triangle throughput.
- **Blade folding.** For short blades, 7 of the 15 verts suffice to describe the shape, so reuse the other 8 to build a *second* adjacent blade — doubling density for the same vertex budget and draw call. Only possible because geometry is procedural.
- **Single quad vs crossed quad vs true blade.** True geometry blades are correct for the near/mid field on this engine — they MSAA cleanly and avoid alpha-test aliasing entirely. Crossed quads / grass cards with alpha textures are only correct for the *far* field impostor tier, and A2C-alpha-tested cards there must be handled carefully (see §4). Single billboards that face the camera are a last-resort distance tier.
- **Rounded normals.** Fake a rounded blade by tilting the interpolated normal outward across the blade width (bending the normal horizontally in the fragment shader). This reads as 3D fullness without extra geometry and is nearly free — do it. Combine with **view-space thickening**: when a blade is edge-on to the camera, shift its verts slightly toward the viewer so it never becomes paper-thin/invisible.

### 3. Lighting grass in a forward renderer

Loom being *forward* is a genuine advantage here — none of the following requires a G-buffer, and the standard deferred-only tricks you'd otherwise be told to use don't apply.

- **The thin-geometry normal problem.** A blade's true geometric normal makes lighting harsh and noisy. Fixes, best-to-cheapest: (1) **outward-tilted/rounded normals** across the width (do this always); (2) **two-sided normal flipping** via `FRONT_FACING`/`VFACE` so back-facing fragments get a flipped normal (`Cull Off` + flip — required because blades are viewed from both sides); (3) **blend the blade normal toward the terrain surface normal**, increasing the blend weight with distance; (4) per-blade normal jitter for variation. **The single most important anti-shimmer lighting trick is #3 pushed hard with distance** — GoT explicitly "gradually blended the outputted normals towards a clump-based common normal as the camera distance increased… reducing noise and gloss," and this is doubly important for you because you have no TAA to clean up the residual specular sparkle.
- **Translucency / SSS.** Cheap wrap-lighting / half-Lambert plus a view-vs-light backscatter term (as in the GodotGrass "hacky" model) is worth it — backlit grass rim-glow is a huge perceptual win for low cost. Do the cheap version; skip anything requiring a thickness texture.
- **Ambient occlusion.** **Darkening toward the blade base is the single highest-value cheap trick** — drive it from model-space height along the blade. This fakes contact darkening with zero SSAO (which you can't do anyway — no post stack). GoT fades this AO out with distance because dark spots in distant grass look unnatural; do the same.
- **Shadows — and the ray-query question.** Do **not** put individual blades in the shadow map (running the full grass pipeline per light is expensive) and do **not** put them in the BLAS/TLAS. This is disqualifying at scale and I can state why concretely: recent 2025–2026 research (Intel "Path Tracing Massive Dynamic Geometry in Jungle Ruins," which reports TLAS updates for >9 million dynamic instances pushing frame times "well beyond the desired 30 FPS" and requiring a partitioned TLAS; and "Ray Tracing Massive Amounts of Animated Geometry," a High-Performance Graphics 2026 Best-Paper honoree) shows that even with heroic engineering, animated foliage forces either per-frame BLAS refits or partitioned-TLAS tricks, and that acceleration-structure updates for millions of dynamic instances are the frame-time bottleneck. The tetrahedral-cage paper reaches ~585M animated triangles at 60 FPS (AMD Radeon RX 9070 XT, 1080p, per AMD GPUOpen) only by *decoupling* animation from the AS and reusing static mini-BLASes — and per the ACM paper the update step (animating cage vertices + rebuilding the TLAS over the tetrahedra) still consumes ~78% of frame time. Millions of independently wind-animated blades needing per-frame AS updates is exactly the pathological case. **Instead:** (a) let grass *receive* the terrain's existing ray-traced sun shadow (sample the same inline ray-query result the terrain fragment uses, or simply the shadow term at the blade base), and (b) fake grass-on-grass/contact occlusion with the base-darkening AO. Ray-tracing a single shadow ray from the blade *base* toward the sun (not per-vertex) is potentially viable since it doesn't require blades *in* the AS — but validate the cost; the cheap analytic darkening is the safe default.
- **Color variation.** Combine per-blade hue/value jitter (from the PCG hash), per-clump color coherence (Voronoi clump color), a vertical base→tip gradient, and a distance color-shift that blends toward the terrain material color. **Clump-level coherence is what defeats the "uniform green carpet" look** — random per-blade variation alone reads as noise; structured (clump) variation reads as real.
- **Wetness & snow.** Integrate with Loom's designed rain system: the deterministic CPU authoritative rain state (intensity + voxel-SDF sky-exposure) already drives wetness accumulation — feed that into a darkening + specular-boost + slight-clumping term in the grass fragment shader (wet grass is darker, glossier, and droops). Sky-exposure gating means grass under an overhang stays dry deterministically. Snow accumulation: bias blade color toward white and add height/mass on up-facing, sky-exposed blades, gated by the same sky-exposure query.

### 4. Aliasing and shimmer without TAA — the central problem

**Why grass aliases.** Four distinct mechanisms: (1) **geometric/coverage aliasing** from thin sub-pixel triangles twinkling as they cross pixel centers; (2) **alpha-test aliasing** at cutout edges (only if you use alpha-tested cards); (3) **specular aliasing** from glossy blade highlights under-sampled per pixel; (4) temporal "crawl" as blades and the camera move. TAA normally hides all four by accumulating jittered samples over time — and you have none of it.

**Non-temporal mitigations, in priority order for Loom:**

1. **Prefer true opaque geometry blades over alpha-tested cards** in the near/mid field. This *eliminates* mechanism (2) entirely and lets MSAA attack mechanism (1) directly on real silhouettes. This is the most important structural decision and it is *easier* for you than for a deferred+TAA engine, not harder.
2. **MSAA 4× (consider 8× given 4090 headroom).** In a forward renderer MSAA anti-aliases geometric edges properly. Cost is real: Unreal's own documentation states "using MSAA instead of TAA increases GPU frame time by about 25%," and general guidance puts 4× MSAA at 10–40% depending on content/bandwidth. On a 4090 at reasonable resolution this is affordable. MSAA does NOT fix specular or (if used) alpha-test shimmer — it must be augmented. (Note the standard "MSAA is dead" claim is specifically about *deferred* renderers where per-sample G-buffer data is discarded before lighting; it does not apply to Loom's forward path — as graphics programmer Alex Tardif argues, "MSAA Isn't Dead… if you have a forward renderer… MSAA is likely readily available for your main pass.")
3. **Alpha-to-coverage (A2C) with a sharpened `fwidth` edge** — but only where you have alpha-tested geometry, i.e. distant cards. Ben Golus's canonical technique ("Anti-aliased Alpha Test: The Esoteric Alpha To Coverage"): `col.a = (col.a - _Cutoff) / max(fwidth(col.a), 0.0001) + 0.5`, plus mip-coverage compensation (`_MipScale ≈ 0.25`, or the "preserve coverage"/Mip Maps Preserve Coverage import option per Castaño/The Witness) so cards don't shrink/vanish with distance. A2C requires MSAA to give more than a binary test. For true geometry blades you generally don't need A2C at all.
4. **Screen-space minimum-width clamping ("sub-pixel coverage compensation").** Don't let a blade get thinner than ~1 pixel; clamp its screen-space width to a floor (Golus: clamp draw width to ~0.5 px) and fade opacity to compensate (Emil Persson's wire technique; Golus's grid-shader variant using `fwidth`). This directly attacks mechanism (1) — the twinkling of sub-pixel blades — and is one of the few things that genuinely helps without temporal data. Combine with A2C in the far field so the opacity fade actually resolves.
5. **Aggressive distance normal-flattening** (see §3) to kill specular shimmer, and **density reduction with distance** while widening surviving blades to keep apparent density constant (GoT drops 3 of 4 blades approaching the far transition).
6. **A single non-temporal post pass is optionally addable.** You have no post stack, but one full-screen compute pass is cheap to add: **CMAA2** (Intel's conservative morphological AA, a DirectX/compute implementation that in the Lumberyard Bistro scene "matched older 4x MSAA edge quality with much less blur and similar rendering speed") or **SMAA 1x** are the credible non-temporal choices. FXAA is cheapest (<1%) but blurs. This is a real option worth prototyping, though it adds a barrier the render graph must own.

**How the AAA references actually do it — and why you can't copy them.** Be explicit: **Ghost of Tsushima's stable image depends on a temporal resolve.** The base PS4 ran native 1080p with a praised temporal AA solution; PS4 Pro used 1800p *checkerboard* reconstruction toward 4K; the 2024 Nixxes PC port exposes SMAA T2x/TAA/DLAA/FSR3 Native AA/XeSS AA. Its grass-specific anti-shimmer (distance normal-blending) sits *on top of* that temporal base. Tellingly, even *with* that temporal machinery, PC players report the far field still fails: on the Steam community, players note "The furthest areas with long white grass show extreme flicker and pop-in and out when the character's camera/view goes up and down quickly," present since release regardless of DLSS/Frame-Gen — direct corroboration that far-field grass crawl is hard even for a temporal engine. Horizon Forbidden West is deferred with a visibility-buffer "deferred texturing" compute path and its own software VRS — entirely TAA/temporal-dependent and G-buffer-based, so essentially none of its AA strategy transfers. UE5.7 Nanite Foliage (Nov 2025, still Experimental) is virtualized-geometry + TSR-dependent. **The takeaway: every modern reference leans on temporal accumulation; your job is explicitly the thing they all avoided doing.**

**Honest verdict.** MSAA (4–8×) + opaque geometry blades + minimum-width clamping + hard distance normal-flattening + density management is **sufficient for a stable, shippable *near/mid* field** and is genuinely better than most people expect. It is **not** sufficient to fully eliminate residual specular sparkle and far-field crawl in motion — that residue is the price of no TAA, it is a genuinely unsolved problem non-temporally, and (as the GoT PC "long white grass flicker" reports show) even temporal engines struggle with it. Adding CMAA2/SMAA 1x as a single post pass closes some of the remaining gap. You must budget for the fact that your still-PNG harness will show a clean image while motion still shimmers — so the golden-image + camera-flythrough harness is effectively a prerequisite for *tuning* this honestly.

### 5. LOD and distance strategy

- **Canonical tiers:** full 15-vert geometry blades (near) → 7-vert blades (mid) → grass cards/billboards with A2C (far) → grass color baked/blended into the terrain material (horizon) → nothing. GoT uses exactly this, with a single terrain texture as the farthest tier.
- **Transition without popping — and which need TAA.** GoT's method: **blend the high-detail blade's vertex positions gradually toward the low-detail shape** before the switch distance so both are near-identical at the swap (no geometric pop), and **thin density gradually** (drop 3 of 4 blades) approaching the tile-size doubling so density already matches across the boundary. Crucially: **dithered/stochastic/screen-door cross-fades between LODs look clean under TAA but reveal their dither pattern without it.** Since you have no TAA, **prefer the continuous geometric-morph + gradual-density approach over dither/screen-door transitions** — this is a specific place the no-TAA constraint changes the right answer. If you must dither, keep the pattern fine and combine with MSAA/A2C.
- **Constant apparent density.** As blade count drops with distance, widen surviving blades to keep the field looking equally dense (standard trick).
- **Distant-grass horizon problem.** Blend the terrain material color toward the grass top-color at distance so there's no hard boundary where blades stop — GoT does this and also fades out base AO at distance.
- **Draw-distance budgets.** Grass geometry typically to a few tens of meters (GoT-class), cards beyond, terrain-color blend to the horizon. Tune to frame budget; the 4090 lets you push the geometry distance further than console references.

### 6. Placement and distribution (integrating with the designed scatter system)

- **Pattern.** For grass specifically, a **jittered grid is usually sufficient** at grass densities — full blue-noise is overkill for the blades themselves. Use Loom's existing **Bridson Poisson-disk / Halton** samplers for clump *centers* and jittered-grid + hash jitter for blades within clumps. This is a good fit: the visually important structure is the clumping, not per-blade blue noise.
- **Clumping.** Voronoi clumping is the highest-value realism lever. Assign each blade to its nearest clump center; the clump drives height, facing coherence, color, and a pull-toward-center. This is what makes a field read as natural rather than as uniform noise, and it directly serves the "avoid uniform green carpet" goal.
- **Terrain-analysis-driven density — Loom's structural advantage.** Because `loom_terrain` already computes slope, flow accumulation, curvature, and hydraulic/thermal erosion, you can *compute* placement rules that other engines hand-paint: lush tall grass in high-flow concave gullies (flow accumulation + negative curvature), sparse grass on steep slopes (slope threshold), none on eroded rock (erosion mask), density falloff near voxel-SDF surfaces. Express these as **flat, schema-validated scalar scatter rules** (TOML `.loom`), never painted masks — matching the AI-authoring constraint. This is the single biggest "made EASIER by Loom" item.
- **Stateless placement from position hashes.** Correct for grass, as argued in §1 — mirror the rain approach. Seeds from quantized world position make dirty-region regen byte-identical.
- **Destructible voxel terrain.** When terrain under grass is destroyed, the placement compute must re-query the voxel SDF each frame (or per dirty region) and **cull blades whose base is now unsupported/floating** (zero-height / no-surface cull stage). Because seeds are position-hashed, regenerating only the dirty region produces identical blades elsewhere. **The trap:** blades floating over a fresh hole for even one frame is a visible artifact; gate blade emission on a current sky-exposure/surface query, not a cached one.

### 7. Culling and performance engineering

- **Cull granularity.** Do **per-tile/per-chunk culling first** (frustum + distance + Hi-Z occlusion on tile bounds), then **per-blade culling in the placement compute** (frustum, distance, orientation/backface, zero-height). GoT found per-blade occlusion culling only marginally beneficial — so do cheap per-blade frustum/orientation culling but don't over-invest in per-blade occlusion; rely on Hi-Z at the tile level.
- **Overdraw is the real enemy.** Grass is many thin overlapping primitives; measured overdraw costs are brutal (one UE4 forum report: a small grass patch at ~8 ms at 1920×1200 on a GTX 560 Ti; a developer report of ~2 ms total for a grass field, "half being drawing the grass to the G-buffer (including depth pre-pass)"). Mitigations: **a depth prepass helps enormously when NOT using MSAA** — that same developer found "depth-only+alpha-test is really cheap, and `GL_EQUAL` depth testing in the second… pass is literally 4 times as fast… Basically doubles my performance, but it isn't usable in the… MSAA pass." Note a prepass interacts awkwardly with MSAA and adds a barrier the render graph owns. **Prefer alpha test / opaque geometry over alpha blending** (blending forces sorting and kills early-Z). Do not sort blades; rely on depth.
- **Published numbers to anchor budgets.** GoT: ~83,000 blades visible, culled from ~1,000,000, in ~2.5 ms on base PS4. UE5.7 Nanite Foliage demo: a reported ~100–120 FPS on an RTX 4080 in a moderately dense scene (still Experimental). These say: on a 4090, grass frame cost is a budgeting choice, dominated by MSAA + overdraw, not by blade math.
- **Mesh/task shaders vs compute+indirect — a real assessment.** In theory, task shaders doing per-cluster culling → mesh shaders emitting blade geometry is ideal (in-pipeline culling, no memory roundtrip, no compaction, no barrier between cull and draw). In *practice on NVIDIA*, the evidence is mixed and cautionary: (a) Hans-Kristian Arntzen's "Modernizing Granite's mesh rendering" (Jan 2024) measured a **~10× slowdown from large task-shader (amplification) outputs**, noting NVIDIA's guidance that the payload "should preferably stay below 108 bytes… under 236 bytes," and that removing hierarchical culling in the task shader made it "similar in performance to plain indirect mesh shading"; (b) a 2026 strand-hair paper found the **mesh-shader LOD variant was actually slower than a compute pre-pass at close/mid range** because a task shader can only launch work at whole-workgroup granularity, whereas a compute pre-pass evaluates per-thread and feeds an indirect dispatch at near-zero cost. **Verdict: start with compute + `vkCmdDrawIndexedIndirect`.** It is the proven grass path, it composes cleanly with Loom's render-graph barrier ownership, and mesh shaders are *not* reliably faster for this workload. Revisit `VK_EXT_mesh_shader` only as a later experiment, not v1. Use **subgroup operations** for append-buffer compaction in the compute pass regardless.
- **Compute→indirect and the render graph.** The compute cull pass writes the blade buffer + indirect args; the draw pass consumes them. This is one compute→indirect-draw dependency (a buffer barrier the render graph inserts automatically). Because Loom's `loom_render_graph` owns all barriers and does automatic placement, you declare the read/write and it handles the `VkBufferMemoryBarrier2` — a clean fit. The indirect-args buffer needs an `INDIRECT_COMMAND_READ` access declared.
- **Scaling to open world.** Stream tiles (GoT double-buffers them), keep one indirect draw per tile, and use position-hashed seeding so tile boundaries are seamless and regen is local. Avoid a frame-time cliff by capping visible-blade count via the density LOD, not by hard distance clipping.

### 8. Wind and interaction

- **Apply Loom's analytic `wind_at(pos,t)` to the Bézier control points.** This is the GoT unified-wind pattern (in GoT, per rendering coder Bill Rockenbeck, "The main wind vector has a constant direction, but we varied the magnitude a bit from place to place using time-varying Perlin noise… visible on large fields of grass when you can see gusts of winds blow through"), and Loom already has the deterministic field with a generated Slang version — a near-perfect fit. Bend the mid/tip control points by the wind vector; keep the base fixed (grass is rooted). **Sample wind at the blade's GLOBAL world position**, never at a chunk-local or model-space position — this is the fix for two of the worst motion artifacts:
  - **Swimming/crawling grass** happens when wind is applied in model space or before the blade's random rotation, so blades appear to slide across the ground instead of swaying in place. (As the hexaquo Godot grass series puts it, applying wind in model space makes "the grass blades… dancing rather than being blown by the wind.") Apply wind bending in world space, to the tip, with the base pinned.
  - **Phase seams at chunk boundaries / unison sway** happen when phase offsets are derived from chunk-local coordinates or a shared clock. Derive per-blade and per-clump phase offset from **global world position** (via the PCG hash of quantized world pos), so there is no discontinuity at voxel-chunk boundaries and no whole-field synchronized bobbing.
- **Gusts as visible waves.** Because `wind_at` already includes sinusoidal gusts + fBm turbulence + a power-law height profile, gusts propagate across a field automatically when sampled per-blade at world position — you get the "gust rolling across the grass" look for free, exactly as GoT got it from time-varying Perlin noise.
- **Interactive flattening/trampling.** Use the **top-down displacement render target** approach: a texture centered on the player that entities write into, sampled by the grass vertex shader to bend blades away. GoT drove theirs from the particle system and, crucially, used a **damped-spring / damped-wave restore** so grass doesn't snap back linearly (verbatim from the PlayStation Blog: "applying a damped wave to the strength of the displacement, which prevents the grass from snapping back to its rest position in a linear and unnatural fashion"). Implement the damped restore; it's a large perceptual win. Handle the map moving with the player via a scrolling/clipmap origin (as Skyrim Community Shaders' grass collision does with an `ArrayOrigin` clipmap wrapping strategy).
- **Determinism of trample state.** Visual displacement (bending) is purely visual → exempt from the sim hash. **But "hiding in grass" is gameplay** — if stealth depends on grass height/occlusion, that query must run on the deterministic CPU side (GoT copied grass height to CPU-accessible data and built physics meshes for stealth). Keep the *visual* trample out of the hash and the *gameplay* occlusion query in it, sourced from the deterministic scatter/height data, never from GPU readback.
- **Wind-driven objects, explosions, vehicle wakes.** All go through the same displacement target (impulses written into it) plus optional radial wind perturbations added to the analytic field — keep authoritative gameplay-affecting forces on the deterministic CPU side.

### 9. Rust / open-source references (honest assessment)

- **Jahrmann & Wimmer, "Responsive Real-Time Grass Rendering for General 3D Scenes" (i3D 2017)** — the academic basis for most open Vulkan grass. Multiple faithful Vulkan implementations exist (UPenn CIS565 projects: byumjin, Rudraksha20, JChunX, DanielZhong; and ACskyline/GodOfFireAndGrass, shineyruan/Vulkan-Grass-Rendering). **Best starting point for raw-Vulkan compute+tessellation grass** — read these for the compute cull + Bézier + culling-test structure (orientation, view-frustum, distance). Caveat: they use tessellation shaders (Loom is dynamic-rendering compute+indirect; you'll adapt away from tessellation).
- **cainrademan/Unity-Grass** (and LogFaer/Unity-URP-Grass fork) — the most readable GoT reimplementation, with an excellent writeup of the exact tricks (clumping, curved normals, distance normal-blending, view-space realignment, blade folding to-do), the `GrassBlade`/`ClumpParametersStruct` layouts, and the AppendBuffer→`CopyCount`→`DrawProceduralIndirect` mechanics. **Read the writeup even though it's Unity/HLSL** — the algorithms transfer directly to Slang.
- **2Retr0/GodotGrass** — GoT-inspired, documents the fake-AO base darkening, horizontal normal-bending, subsurface hack, and honestly notes its tile-based LOD *pops* ("LOD swapping is very noticable due to the tiled nature of the system") — a cautionary data point for your no-TAA transition problem.
- **bevy_procedural_grass** (jadedbay) and **bevy_feronia** — Rust/bevy compute-shader grass. `bevy_procedural_grass` uses compute shaders to generate instance data per frame and takes a segment count for the mesh; `bevy_feronia` is a declarative scatter+wind crate explicitly crediting the GoT talks. **Most directly relevant to a Rust engine**, though bevy's wgpu abstraction differs from Loom's raw ash/Vulkan — treat as design reference, not drop-in.
- **giordi91's grass shader** — vertex+fragment, tile-based, GPU-driven culling+LOD, indirect rendering, no geometry/tessellation shader — architecturally the closest to what Loom should build, with a good discussion of overdraw and why mesh shaders looked appealing.
- **GPUOpen "Procedural grass rendering — Mesh shaders"** (AMD) — if you ever prototype the mesh-shader path, this is the reference (one mesh-shader threadgroup per grass patch, building on the Jahrmann & Wimmer tessellation idea). Note the NVIDIA task-shader caveats in §7.

### 10. What to skip (with reasons)

- **TAA / motion vectors / temporal upsampling (DLSS/FSR/XeSS)** — explicitly out of scope by constraint. Don't design anything that assumes them.
- **Deferred / visibility-buffer grass (Horizon-style deferred texturing)** — requires a G-buffer Loom doesn't have; forward + MSAA is your path.
- **Nanite/virtualized-geometry grass (UE5.7)** — requires infrastructure Loom lacks and is still Experimental; not worth reimplementing solo.
- **Grass in the ray-tracing acceleration structure / ray-traced per-blade shadows** — disqualified at scale (see §3); the AS-update cost for millions of animated instances is the documented bottleneck. Tetrahedral-cage / partitioned-TLAS research is impressive but far beyond solo scope.
- **Blades as ECS entities** — pathological given `loom_ecs` is `Vec<Option<T>>`; blades are transient GPU data, never entities.
- **Node-graph material/scatter authoring** — a documented poor fit for LLM authorship in this project; use flat scalar TOML rules.
- **Painted density masks** — not acceptable as authored content; use polygon/spline/op-list scatter rules driven by terrain analysis.
- **GPU→CPU readback for anything gameplay-affecting** — disqualifying for determinism; compute stealth/occlusion from deterministic CPU scatter data instead.
- **Full order-independent transparency / alpha-blended grass** — sorting/OIT is expensive and unnecessary; opaque geometry + A2C is correct. (Note Ben Golus's warning that alpha-blended intersecting foliage "almost looks like the bush is inside out" in motion because it can't depth-sort per pixel.)

---

## Recommendations

**Dependency ordering (what must exist first):**
1. **Wind field (`wind_at`)** — prerequisite. Grass animation is built directly on it. It's designed but not yet built; build/land it first.
2. **Procedural scatter system** — prerequisite for placement. Grass placement *is* a scatter consumer (clump centers + blades). Its position-hash seeding, Poisson/Halton samplers, and dirty-region regen are exactly what grass needs. Land scatter before grass.
3. **Golden-image regression harness + deterministic camera flythrough** — effectively a prerequisite for *tuning* the no-TAA AA, because shimmer/popping/swimming are invisible to a still PNG. It's already your planned next work item; prioritize it, and add short animated-sequence captures (not just stills) so motion artifacts are reviewable.
4. **Prefab system** — NOT a hard prerequisite for grass (grass isn't a prefab), but useful for authoring test scenes. Can proceed in parallel.
5. Render-graph compute→indirect support (barrier declaration for indirect-args buffer) — small, do as part of grass.

**Staged plan:**

- **v1 — "minimal but convincing," achievable without TAA (~3–4 dev-weeks with AI agent):**
  Compute placement/cull → append buffer → `vkCmdDrawIndexedIndirect`; 15/7-vert Bézier geometry blades; **opaque geometry only, no cards yet**; Voronoi clumping; rounded normals + two-sided flip + base-darkening AO + cheap wrap-lighting; wind from `wind_at` applied in world space to tip control points with per-blade/clump phase from global-position PCG hash; **MSAA 4×**; distance normal-flattening; terrain-analysis-driven density (slope/flow/erosion); position-hash stateless placement. Verify with the camera flythrough, not just stills. This alone looks good near/mid and is stable.
- **v2 — distance + interaction (~3–4 dev-weeks):**
  Add the LOD tiers (7-vert → A2C cards → terrain-color blend) with **geometric-morph + gradual-density transitions (no dither)**; minimum-width screen-space clamping; blade folding; top-down displacement target with damped-spring restore; wetness from the rain system; destructible-voxel dirty-region regen with floating-blade culling.
- **v3 — polish (~1–2 months):**
  Prototype CMAA2 or SMAA 1x as a single non-temporal post pass (if the residual specular shimmer is objectionable in motion); Hi-Z tile occlusion culling; snow accumulation; per-blade ray-queried sun shadow from the base (only if profiling shows headroom); tune open-world streaming and per-tile budgets.

**Benchmarks/thresholds that change the plan:**
- If MSAA 4× costs >~15–20% of frame at target resolution, drop to 2× + rely harder on normal-flattening + a post AA pass rather than 8×.
- If overdraw dominates the profile, add a depth prepass (accepting the MSAA interaction cost) or reduce near-field density/draw distance before touching blade geometry.
- If motion shimmer remains objectionable after v2 despite MSAA + normal-flattening + min-width clamp, that is the signal to add the CMAA2/SMAA post pass (v3) — this is the one place you may need to relax "no post-process stack" to a single additive full-screen pass.
- If per-tile culling leaves the compute pass as the bottleneck (unlikely on a 4090), *then* evaluate the mesh-shader path — but only then.

---

## Caveats and failure modes (the highest-value section — read before implementing)

**Things that look correct in a still PNG but are wrong in motion (your automated verification is blind to all of these — this is the structural risk):**
- **Sub-pixel blade shimmer/twinkle** — blades crossing pixel centers flicker. A still shows a clean frame; motion crawls. Mitigate with min-width clamp + MSAA + density falloff; accept residue without TAA. (Even TAA-equipped GoT PC shows this in the far "long white grass" — it is genuinely hard.)
- **Specular sparkle on glossy/wet blades** — MSAA does not touch it. Distance normal-flattening is the main lever; a still won't reveal it.
- **LOD popping** — a still at one distance looks fine; crossing the transition pops. Use geometric morph + gradual density, NOT dither cross-fade (dither needs TAA to look clean).
- **Unison sway** — all blades bobbing in phase looks like a stadium wave; only visible in motion. Fix with global-position-derived per-blade phase.
- **Swimming/crawling grass** — blades sliding across the ground instead of swaying. Caused by model-space wind or wind applied before per-blade rotation. Apply wind in world space to the tip with the base pinned.
- **Phase seams at voxel-chunk boundaries** — a visible line where phase resets. Caused by chunk-local phase seeds. Derive phase from global world position only.
- **Density popping** — blades appearing/disappearing in clumps as tiles change LOD. Fix with gradual 3-of-4 density thinning before the boundary.

**Things that look correct but are subtly wrong (visible in stills too):**
- **Clump repetition** — too few Voronoi cell types or a low-period hash makes visible repeating patterns. Use a high-quality hash (PCG) and enough clump variety.
- **Normal artifacts at grazing angles** — without two-sided normal flipping, back-facing blades light black; at grazing angles blades vanish without view-space thickening.
- **Blades intersecting geometry / on vertical surfaces** — grass on a cliff face or poking through rocks. Gate placement on slope (from terrain analysis) and on voxel-SDF proximity.
- **Uniform green carpet** — insufficient clump-level color/height coherence; per-blade random alone reads as noise, not variation.
- **Hard grass/terrain horizon boundary** — no terrain-color blend at distance; grass just stops.

**Things that only break at scale (millions of blades):**
- **Append buffer overflow** — size for worst-case visible count with margin; a too-small buffer silently drops blades (a subtle, load-bearing bug). Clamp the indirect draw count to buffer capacity.
- **Overdraw cliff** — near-field dense grass tanks fill rate; the triangle count is fine, the shaded fragments are not. Budget by overdraw, not blade count.
- **Indirect-args race** — if the render graph doesn't barrier the compute→indirect dependency correctly you get garbage/last-frame counts; declare `INDIRECT_COMMAND_READ` explicitly.
- **MSAA memory** — 4–8× MSAA render targets at high res consume large bandwidth/memory; can be the real cost, not the geometry.

**Things that break determinism silently (the insidious ones):**
- **Any per-blade value derived from dispatch/thread order** instead of position — makes dirty-region regen non-identical. Always seed from quantized world position.
- **GPU→CPU readback feeding gameplay** (e.g., reading grass height back for stealth) — readback timing is non-reproducible → disqualifying. Compute gameplay-relevant grass state on the deterministic CPU side from the same scatter data.
- **Floating-point drift between the Rust `wind_at` and the generated Slang version** — if they diverge, CPU gameplay and GPU visuals desync. Keep the Slang generated from the Rust source (Loom's existing pattern) and test agreement; be wary of `fma`/precision differences between the debug and release builds that `cargo xtask validate` checks.
- **Trample/displacement leaking into the hash** — keep visual displacement out of the sim hash; keep only the deterministic gameplay occlusion query in it. Conflating them either breaks determinism (if GPU-sourced) or wastes hash budget.
- **HashMap/HashSet iteration or `thread_rng` sneaking into placement** — banned by clippy.toml workspace-wide; scatter must use ordered, seeded structures.

**Destructible-terrain-specific:**
- **Floating blades over fresh holes** — grass whose base voxel was just destroyed hangs in the air for a frame. Gate emission on a *current* surface/sky-exposure query each frame for dirty regions, not a cached one.
- **Dirty-region regen non-determinism** — only safe because seeds are position-hashed; verify a destroyed-then-regenerated region byte-matches an initially-generated one.

**AA verdict restated as a caveat:** there is genuine industry disagreement and no clean solution here. MSAA + opaque geometry + min-width clamp + normal-flattening is the best non-temporal stack available and is better than the "MSAA is dead" conventional wisdom suggests (that wisdom is about *deferred* engines; you are forward). But anyone claiming you can fully match a TAA-resolved grass field's motion stability without temporal accumulation is wrong — and even temporally-resolved shipping games (GoT PC's far-field "white grass flicker") don't fully solve it. Budget for a residual, and budget for the golden-image + flythrough harness being the only way you'll see it.