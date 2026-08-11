# Realistic and Performant Rain in Loom: An Actionable Implementation Plan

## TL;DR
- **Build rain as a stateless, analytic GPU system (hash-based particle positions from `pcg2d`/`pcg3d` per Jarzynski & Olano) that is NOT part of the deterministic sim**, mirroring your analytic-Gerstner-waves decision; keep a tiny CPU-side authoritative "rain intensity + sky-exposure query" that IS deterministic and drives gameplay. This is the single most important architectural decision and it sidesteps GPU→CPU readback entirely.
- **Occlusion is what makes rain look real.** Use your existing hardware ray queries for occlusion (a short `terminateOnFirstHit` ray upward per splash/drop) because you already maintain a BVH and because destructible voxel terrain invalidates a baked top-down depth map — the ray-query approach is the correct answer *specifically for this engine*, whereas a top-down "rain occlusion buffer" would be the answer for a static-geometry engine.
- **The absence of a post-process stack is the biggest constraint.** It rules out screen-space raindrops-on-lens, SSR-based wet reflections, and the classic Tatarchuk fullscreen-composite rain. Your convincing v1 is therefore: stretched camera-locked billboard streaks + ray-query occlusion + wetness-darkening in the forward material + splash/ripple decals driven by the water system you already designed. Budget ~3–5 developer-weeks with an AI agent for a strong v1.

## Key Findings

1. **There are two historically dominant families of rain rendering, and one of them is closed to you.** The Tatarchuk/ATI "ToyShop" (SIGGRAPH 2006) approach is fundamentally a **fullscreen post-process composite** ("Rendering Multiple Layers of Rain with a Post-Processing Composite Effect", ShaderX5) layered on top of stretched particles. Without a post-process stack you cannot do the composite half. The other family — **camera-locked stretched-billboard particles** — is fully available to you and is what most shipping games actually rely on for the falling-rain layer.

2. **Rain does not need to be deterministic, and trying to make it deterministic is a trap.** The perceptual research (Garg & Nayar; Tatarchuk & Isidoro) shows humans cannot track individual drops and cannot even recognize rain from a single static frame — so exact drop positions are gameplay-irrelevant. Decouple the visual particles (GPU-only, stateless, never read back) from a cheap deterministic CPU scalar state (is-it-raining, intensity, wind vector, per-query occlusion). Gameplay/replay/verification only ever touches the CPU scalar state.

3. **Stateless particles solve determinism, readback, and per-particle storage simultaneously.** A raindrop's position is a closed-form function of its integer index and current time: `pos = hash(index) → spawn cell; y = wrap(startY - speed*t)`. No stored state, no simulation buffer, no readback, no CPU→GPU upload beyond a time uniform. Use `pcg2d`/`pcg3d` (Jarzynski & Olano, JCGT 9(3):21–38, 2020, which concludes "pcg3d and pcg4d fall on the Pareto Frontier and are a good default choice for multidimensional high-quality hash functions; xxhash32 would be a good default"). Nathan Reed's companion blog is blunter: the older Wang hash "did not lie along the Pareto frontier—not even close! The solution that dominates it—and one of the best balanced choices between performance and quality overall—is PCG."

4. **Your terrain flow-accumulation data is the right input for puddle placement** and is a genuine competitive advantage — Lagarde explicitly uses heightmap cavities ("black mean hole where water can accumulate") to place puddles; you already compute hydraulic erosion, flow accumulation, and curvature, which is strictly better data.

5. **Ray-query occlusion is cheap to trace but the BVH is the real cost** — and destructible terrain means topology changes force full BLAS *rebuilds*, not cheap refits. This is the central performance nuance for your engine.

## Details

### 1. Rain particle rendering techniques — the full landscape

**Camera-locked / view-space rain volume (RECOMMENDED core).** Spawn drops in a volume (box or cylinder) centered on and following the camera. The standard dimensions are a ~20×20 m footprint (multiple independent sources converge on this: the Unity/URP community, the "Game Particle Effects" 2026 guide, and Lagarde's 20 m ortho frustum for the related depth map). The wrapping math is the key trick: each drop has a fixed XZ from `hash(index)` inside the disk/box and a height `y = worldTopY - mod(speed*t + hash2(index)*range, range)`. Because you `mod` the fall distance, drops "recycle" forever with zero CPU involvement (the Three.js writeup by Peter Adams describes exactly this: "it uses shader math to recycle drops vertically so they appear to fall forever without respawning on the CPU"). **Seam-avoidance trap:** if the volume rigidly follows camera XZ, drops appear frozen relative to the world when the camera translates; the fix is to keep the volume world-anchored and only re-anchor (snap by a full cell) when the camera leaves the current volume, so wrapping is continuous.

**GPU instanced quads vs GPU-driven indirect draws.** For rain's particle counts, a plain instanced draw with per-instance index (using `gl_InstanceIndex`, positions computed analytically in the vertex/mesh shader, vertex data pulled via buffer device address) is the correct choice. Indirect draw only earns its keep when the particle *count itself* is computed on the GPU (append/consume buffers) — which you do NOT need for stateless rain because the count is a known constant per intensity level. Point sprites benchmark fastest for particles historically (Geeks3D: "Point sprites are the fastest way to render a lot of particles"), but they clip when the sprite center leaves the screen and cannot be stretched into streaks, so use two crossed camera-facing stretched quads per drop (the standard technique; the Unity geometry-shader tutorial builds exactly this cross to avoid the edge-on invisibility artifact).

**Streak/stretched billboards.** Orient the quad along the drop's world-space velocity projected into screen space, and stretch length proportional to `velocity * exposure_time` (Garg & Nayar's photometric model formalizes this — a streak is a motion-blurred drop over the camera's integration time). Bake the motion-blur *into the texture* (a soft head-to-tail gradient) rather than computing it — this is what the GameDev.net analysis of ToyShop concluded ("a single texture which was accumulated and motion blurred… a higher rain velocity means a higher blur distance per frame").

**Rain as fullscreen/screen-space effect.** This is the Tatarchuk composite and the "4 sliding texture planes" flight-sim trick. **Skip it** — it requires a post-process stack you don't have, and the scrolling-texture variants break badly when moving in/out of buildings.

**Garg & Nayar photometric research — practical or not?** Their rain-streak *database* (20 streaks × 10 oscillations, indexed by lighting direction θ_light, φ_light and view direction θ_view) is the gold standard for offline/vision work and is used in autonomous-driving synthesis pipelines (arXiv 2502.16421, 2009.03683). For a real-time solo project it is **not worth it**: Weber, Jolivet, Gilet & Ghazanfarpour ("A multiscale model for rain rendering in real-time," *Computers & Graphics* vol. 50, 2015) note such a database "has a high memory footprint and also requires complex mechanisms to control particles, in order to preserve a constant and physical distribution of raindrops in space." Take the *insight* (drops are motion-blurred, oscillating, lighting-dependent streaks) and bake 2–4 streak textures, not the ~200-entry database.

**Physically-based drop-as-lens refraction.** Individual drops act as lenses refracting the environment. In games this is only ever done for **drops on glass/lens** (screen-space refraction of the scene color buffer), never for falling drops. Without a post-process/scene-color stack you can't do the screen-space version. **Skip for falling rain**; revisit only if you later add a windshield/visor effect.

**How many particles?** Convincing rain needs far fewer than intuition suggests because of layered depth. Typical counts: a few thousand up to ~10,000 stretched drops for heavy rain in the camera volume (Unity community sets "Max Particles… 10,000"). The perceptual tricks that reduce the number: near/mid/far layers with the far layer as a scrolling texture or larger, fainter, slower streaks; opacity and speed falloff with distance; and soft depth-fade so drops vanish against near geometry (Dontnod/Remember Me: "a soft depth test can… progressively decrease the opacity of raindrops," which also makes drops "disappear when looking at the ground").

### 2. GPU vs CPU simulation and the determinism question

**Does rain sim need to be deterministic?** No — with one caveat: anything that feeds gameplay must be. Split the system:

- **Visual layer (GPU, non-deterministic, never read back):** stateless streaks + splashes. It can freely use frame time and GPU hashing. Nothing here touches the sim hash.
- **Authoritative layer (CPU, deterministic, in the fixed-timestep sim):** a small struct — `intensity: f32`, `wind: Vec3`, and a way to answer "is world-point P exposed to sky?" This is what `loom sim … --assert` sees. Because it's a handful of scalars plus analytic queries, it hashes deterministically and respects your clippy bans (no HashMap iteration, no `thread_rng`, no `Instant::now`).

**What breaks if you decouple?** Only things that need exact drop↔world correspondence: e.g., "this specific drop extinguished this specific fire." The industry answer (Lagarde, Tatarchuk) is that you **never** need this — splashes are decoupled from drops ("it is simpler to have two independent systems to manage raindrops and rain splashes"). Gameplay effects key off the intensity scalar and the occlusion query, not off particles.

**Stateless hashing — specific functions.** From Jarzynski & Olano (JCGT 9(3), 2020):
- **`pcg` (32-bit), `pcg2d`, `pcg3d`** — the recommended default. Best balance of TestU01 quality and GPU speed. `pcg2d` fits rain perfectly (2D cell index → 2D random offset).
- **`xxhash32`** — the paper's alternate default; higher quality than the 2D plots suggest, slightly more expensive.
- **`iqint` / Inigo Quilez integer hashes** — cheap, decent, good for non-critical jitter.
- **Wang hash / Hugo Elias hash** — legacy; off the Pareto frontier. Avoid.
- **`hashwithoutsine` / trig hashes** — banding artifacts; avoid.

**Mirroring the water approach for CPU gameplay sampling.** Yes — this is the cleanest fit with your architecture. Author the rain formula once in Rust (intensity, wind, spawn distribution constants), generate the Slang GPU version from that Rust source (exactly your Gerstner-wave anti-divergence pattern), and expose CPU functions `rain_intensity_at(p, t)` and `is_exposed(p)` for gameplay. The CPU side does NOT need to evaluate individual drops — only the analytic intensity field and the occlusion query.

### 3. Occlusion — the thing that makes rain look real or fake

**The depth-map approach (industry standard, but wrong for you).** Render an orthographic depth map from above along the rain direction; use it to (a) kill/fade particles under cover and (b) place splashes on the topmost surface. Lagarde gives concrete numbers: "With a 256×256 depth map and a 20m x 20m orthogonal frustum we get world cells of 7.8cm² at the height taken from the depth map." Cost: "On PS3 a 256×256 depth map rendering mainly dominated by character take around 0.32ms the rain splashes under heavy rain take around 0.33ms. On XBox360 depth map take around 0.20ms the rain splashes under heavy rain take around 0.25ms." GTA V, Batman: Arkham Knight, and the ToyShop demo all use variants of this top-down depth/occlusion buffer. **The problem for Loom:** a top-down ortho map handles overhangs poorly (single depth per cell) and a *baked* map is invalidated the instant the player blows a hole in a ceiling with your destructible voxels.

**Ray queries are the better occlusion answer for this engine — here is the quantified case.** You already have `VK_KHR_ray_query` and a BVH for sun shadows. Per-drop or per-splash, trace a short ray toward the sky with `gl_RayFlagsTerminateOnFirstHitEXT` against opaque-flagged geometry (NVIDIA best practices: terminate-on-first-hit + opaque flag is the canonical cheap-occlusion path; and "Don't include sky geometry in TLAS"). Cost data:
- **Tracing itself is cheap.** Tellusim's "Ray Tracing Performance Comparison" (Sept 24 2021) shows an RTX 3080 tracing primary+shadow+reflection rays (3/pixel) at 1600×900 in **0.55 ms** via Ray Query Vulkan (compute-shader ray tracing is "7.7 times slower than HW accelerated"). A 4090 has ~2× the RT-core throughput. Boolean occlusion rays are cheaper still (no closest-hit sort, no hit shading). Tens of thousands of drop/splash occlusion rays are effectively free.
- **The real cost is BVH maintenance, and this is the trap for destructible terrain.** NVIDIA: BLAS *updates* (refits) are cheap "after limited deformations," but "topology changes in an update mean triangles degenerate or revive" — i.e. **voxel destruction forces full BLAS rebuilds, not refits.** Tellusim (81 instances of 490K triangles): GeForce 3080 "BLAS Build" = **7 ms fast-build / 18 ms fast-trace** (compacted BLAS 33 MB / 15 MB). NVIDIA's own guidance is to keep AS build/update ≤2 ms and to "distribute rebuilds over frames." UE5 echoes: deforming-mesh BLAS rebuilds are "proportional to the total number of triangles being deformed."
- **Verdict:** ray-query occlusion is the right call *because you are already rebuilding/maintaining the terrain BVH for sun shadows and voxel destruction anyway* — the marginal cost of rain occlusion rays is just the (cheap) trace. If you were NOT already maintaining that BVH, the depth map would win. Honesty note: NVIDIA confirms there is **no official rays/second figure for the 4090** ("no published gigarays numbers since Turing… measure your own application") and no public absolute register-count for ray-query objects — profile with Nsight/your own timing.

**Register-pressure caveat for tracing from the fragment shader.** Inline ray queries hold traversal state in registers and can cut occupancy (NVIDIA: "query objects must hold state… this consumes registers and complex user code may limit occupancy sooner than usual"). For rain, prefer tracing occlusion in a **compute pass** that produces a small splash-spawn list / exposure mask, rather than tracing per-fragment, and use compile-time ray flags.

**Interior/exterior detection for gameplay.** The same `is_exposed(p)` occlusion query answers this deterministically on the CPU: for the CPU sim, do a cheap analytic/voxel-raymarch upward through your i8 SDF chunks — you already have the SDF, so a CPU sky-visibility march is deterministic and needs no GPU. This is a nice bonus: the voxel SDF gives you a *deterministic CPU* occlusion answer for gameplay without any GPU readback, while ray queries give the *visual* GPU answer.

### 4. Surface response — wetness, puddles, ripples, splashes

**Wetness (do this in the forward material — it's the highest realism-per-effort item).** The Lagarde/Dontnod model, corroborated by the "Water drop 3" posts and Uncharted 4's "Wetness Shading" (Naughty Dog, "The Technical Art of Uncharted 4," SIGGRAPH 2016): when wet, **darken and saturate albedo, reduce roughness (boost specular), and optionally add a thin water-film normal**. The physically grounded version keys the darkening on **porosity** ("only porous materials are affected by rain"; "low albedo, rough and porous materials tend to have larger wetting effect"), but Lagarde concludes a monochromatic roughness/porosity factor is sufficient and cheaper than per-channel albedo curves. For an AI-authorable, schema-validated system, expose flat per-material params: `porosity: f32`, `wet_darkening: f32`, `wet_roughness_mul: f32`. Most games (Stalker, Uncharted, AC3, Crysis, MGSV) use eye-calibrated factors — Lagarde notes AC3's per-rain-type strength change is "a wrong step" physically, so drive wetness by accumulated water + exposure, not by rain type.

**Accumulation and drying.** Drive a per-surface wetness scalar up by `intensity * exposure` (gated by the occlusion query) and down by a drying rate. Key realism note from fxguide/Lagarde: **specular wetness vanishes faster than diffuse darkening when drying** ("when drying, specular strength disappears faster than the darkening of the diffuse"), and drying is non-homogeneous. A two-rate model (fast specular decay, slow albedo recovery) captures this cheaply. This maps naturally to your existing systems as a value that accumulates over sim ticks.

**Puddles — use your terrain flow-accumulation data.** This is a real edge. Lagarde places puddles from heightmap cavities; you have hydraulic erosion + flow accumulation + curvature already. Flow accumulation identifies convergence zones (where water pools); low curvature + high flow accumulation = puddle. Bake a puddle mask from these at terrain-gen time, and let it feed both the wet-material blend and (optionally) the height-field ripple sim you already designed for water. **Trap:** flow accumulation gives *where water would collect over a large area*; you still want a slope/flatness gate (your walkability/slope analysis) so puddles don't smear up gentle inclines.

**Ripples — normal-map atlas vs height-field sim.** For 95% of surfaces, a **scrolling/flipbook rain-ripple normal atlas** is the right cost (Unity HDRP's production Weather sample combines four `RainRipple` instances at different scale/phase; Cyanilux generates ripples procedurally in-shader via a Voronoi-like cell method — no texture needed). Reserve the **actual height-field wave-equation sim** (your `ü = c²∇²u + cα∇²u̇`) for hero puddles/water surfaces where you want drops to seed real ripples — Tatarchuk did exactly this ("rendered seeds act as the initial ripple positions… exciting the ripple propagation in the subsequent passes"). Since you already designed the shallow-water ripple sim, seeding it with rain impacts is a natural, cheap extension for hero water.

**Splashes — decouple from drops (universal industry practice).** Do NOT collide 10,000 drops. Spawn splashes from occlusion-query hit points (or a random distribution over the exposed near-camera region), as independent particles. Lagarde: "it is simpler to have two independent systems"; the count is tied to the intensity scalar, "only generated close to the screen." Use a scaled quad with a baked splash/crown animation (ToyShop used a single milk-drop high-speed-video texture, randomly flipped horizontally). Corona vs prompt splash detail is not worth modeling — a crown mesh + droplet sprite is the standard.

**Drip/runoff from edges.** Scrolling normal maps displaced in the world-down direction, gated by the sky-occlusion factor (the GameDev.net wet-shader thread describes exactly this: "additional scrolling normal maps for dripping water, displacing them in the approximated 'down' direction… fade the impact… using sky hemisphere occlusion"). Unity's Weather sample has dedicated `Rain_Drips` subgraphs keyed on material permeability. Low priority for v1.

### 5. Lighting and atmosphere

**Drops catching light.** Rain visibility is strongly lighting-dependent — backlit rain reads far more than frontlit (Garg & Nayar; the RDR2 "milk rain" bug mod shows what happens when this is wrong: drops render as bright milk-white against dark backlit scenes). The cheap, well-attested trick (GameDev.net wet-shader thread): push scene lighting into a low-res 3D volume texture, blur it, and texture-fetch it per drop so drops pick up nearby headlights/street-lamps. Since you have a forward renderer with light lists, sampling the N nearest lights per drop in the vertex shader is also viable at rain's counts.

**Fog/volumetric and attenuation.** Heavy rain reduces visibility (atmospheric attenuation) and adds misty halos around lights. You have **no volumetric fog** — do not build it for rain. A cheap approximation: increase your existing distance fog density and desaturate with intensity; add billboarded "light-shaft/halo" sprites around bright lights if needed. Flag: this is an approximation, not physically-based scattering.

**Wet reflections without a post-process stack — the honest gap.** Wet scenes lean heavily on reflections (SSR/planar), and you have neither SSR (needs post-process/scene-color) nor a planar reflection system described. Your available substitutes: (1) **boosted specular + reduced roughness** in the wet material (gives the "wet sheen" from direct lights and any IBL/environment cubemap you have), and (2) **ray-traced reflections via your existing ray queries** for hero wet surfaces (puddles) — this is actually a strong option you uniquely have, and it sidesteps SSR's inability to reflect off-screen/occluded detail. Trace a reflection ray from puddle pixels; cost is modest given your existing BVH. This is the single place where "no post-process stack" hurts most, and ray queries are your escape hatch.

**Lightning.** Optional. Tatarchuk treats it as a strong directional light affecting the whole scene with a matching shadow update, plus a screen flash. Doable in your forward renderer as a transient directional light; the flash-brightening would normally be a post-process, so approximate by scaling light intensity. Low priority.

### 6. Audio

**Standard rain audio:** layered loops (base rain loop + surface-specific textures + individual near-field drop/impact sounds + thunder), with **intensity crossfading** driven by an RTPC-style parameter (Audiokinetic's Wwise rain example uses a `Rain_intensity` parameter that "changes many of the settings for rain, filter settings, and the volume"), **occlusion-driven low-pass filtering** for interior vs exterior, and surface-material-dependent sounds (rain on tin vs leaves vs puddle — the ACM TOG "Physically-based statistical simulation of rain sound" paper models this with material sound textures, but that's research-grade). For a solo dev: 2–3 loops crossfaded by intensity + a low-pass when `is_exposed` is low + a few positional impact sounds.

**Does your geometry-traced acoustics system help?** Yes, specifically and elegantly. Your audio system already ray-traces real scene geometry for acoustics — so the interior/exterior mixing and occlusion filtering that other engines fake with hand-placed volumes falls out for free: rain audio should be occluded and low-passed by the *same* traced geometry that occludes the rain visually and the sky-exposure query. Concretely, drive the rain-loop low-pass cutoff and volume from the same sky-visibility fraction you compute for occlusion. This is a genuine synergy worth calling out: one occlusion concept (sky exposure) feeds particles, wetness, splashes, gameplay, AND audio.

### 7. Performance engineering

**Budgets.** Published rain-system costs: Lagarde's PS3/360 occlusion+splash system totaled well under ~1 ms on 2012 consoles (0.32 ms + 0.33 ms on PS3); on a 4090 the entire rain system (streaks + occlusion rays + wetness + splashes) should sit comfortably in a low-single-digit-ms budget if fill rate is controlled. The dominant cost is **not** particle sim (stateless = ~free) or ray tracing (cheap) — it's **overdraw**.

**Overdraw/transparency is THE fill-rate problem.** Thousands of overlapping alpha-blended streaks = massive overdraw ("if these effects fill the screen, overdraw can be almost unbounded" — GPU Gems 3 ch.23, "High-Speed, Off-Screen Particles"). Mitigations, in order of value for you:
- **Additive or premultiplied blending to avoid sorting.** Rain streaks are bright-on-dark; additive blending is order-independent (no per-particle sort needed) and cheaper than alpha blend. This is the single biggest win and it also eliminates a determinism-irrelevant but complexity-heavy sort.
- **Keep the shader trivial** (one texture fetch, no per-drop lighting math in the worst case) since it runs per-overdrawn-pixel.
- **Depth-test against the scene depth** (soft depth fade) so drops don't overdraw behind opaque geometry.
- **Alpha-to-coverage / MSAA:** A2C gives order-independent cutout transparency and pairs with MSAA, useful for the crisper near streaks; but it's better for foliage-like cutouts than faint streaks — modest value here.
- **Half-resolution transparency pass + composite:** GPU Gems 3 ch.23 shows big overdraw savings rendering particles to a fraction-res target. **But this needs a composite step (a mini post-process)** you don't have yet, and it introduces depth-edge artifacts. Defer until you have a compositing path.

**Reduced-resolution rain compositing.** Same as above — worth it eventually, but it's effectively a post-process and you lack the stack. Artifacts: soft haloing/edge bleed where low-res rain meets high-res depth edges. Defer.

**Vulkan-specific notes.**
- **Instanced draw + buffer device address** for per-instance constants is the right baseline; positions computed analytically from `gl_InstanceIndex` so the vertex buffer is just a unit quad.
- **Indirect draw** only if you later want GPU-decided counts; not needed for constant-count stateless rain.
- **Mesh/task shaders:** NVIDIA explicitly notes particles are "sparse topology" and mesh shaders can work for them, but for simple stretched quads the win over instancing is marginal and adds complexity. **Skip for v1**; a mesh-shader path is a nice later optimization for combined cull+expand.
- **Subgroup ops** for culling/compaction only matter if you're compacting a variable particle set — stateless rain doesn't need it.
- **GPU-driven particle systems** (append/consume, indirect) are overkill at rain's counts and for stateless rain specifically; their benefit is state-preserving million-particle sims.

**LOD and drizzle→downpour scaling.** Scale particle count and splash count linearly with intensity; scale streak length with fall speed; fade the far layer first. To avoid a frame-time cliff at "torrential," **cap the near/full-detail volume and push extra intensity into the cheaper far texture layer and into fog density / audio / wetness** rather than into unbounded particle counts. This keeps worst-case overdraw bounded.

### 8. Rust ecosystem

- **Hashing/procedural placement:** You do NOT need a crate for the GPU side — port `pcg2d`/`pcg3d` directly into Slang (a dozen lines; MIT-licensed reference at `github.com/markjarzynski/pcg3d` and the shadertoy XlGcRh). For the CPU authoritative side, implement the *same* PCG in Rust so CPU and GPU agree (again mirroring your Gerstner approach). This guarantees determinism and avoids depending on `rand`.
- **Noise crates (if you want wind gust fields or spatial intensity variation):** `noise` (Razaekel/noise-rs, mature, the de-facto standard) or `noise-functions` (uses static permutation tables/hashing, `f32`, no per-instance state — a good fit for stateless deterministic sampling). `simdnoise` is fast but older (last significant activity ~6 years ago). `bracket-noise` (FastNoise port) is fine and lightweight. **Honest maturity read:** `noise-rs` is the safe, well-maintained choice; `noise-functions` is newer but architecturally cleaner for your stateless/deterministic needs. Verify any crate is deterministic across platforms and doesn't internally use `HashMap` iteration or RNG that would trip your clippy bans.
- **Open-source rain implementations worth reading:** the Three.js "Cheap, Beautiful Rain" writeup (cylindrical volume + shader vertical recycling — directly maps to your stateless approach), Cyanilux's Rain Effects Breakdown (in-shader Voronoi ripples, no atlas), Unity's HDRP production Weather Shader Graph sample (puddle/ripple/drip subgraph decomposition is a good schema blueprint), and NVIDIA's D3D10 "Rain" SDK sample (older, particle-focused). No standout production-grade *Rust/Vulkan* rain reference exists — this is greenfield, so lean on the technique papers, not on porting a codebase.

### 9. What to skip (with reasons)

- **Fullscreen composite rain (Tatarchuk ShaderX5)** — needs a post-process stack. Skip.
- **Screen-space raindrops-on-lens / drop-as-lens refraction** — needs scene-color post-process. Skip until you have a windshield/visor use case.
- **SSR for wet reflections** — needs post-process/scene-color; use boosted specular + ray-traced reflections instead.
- **Garg & Nayar streak database (~200 entries)** — research-grade, high memory, complex control. Bake 2–4 streak textures instead.
- **Physically-based splash (corona/prompt, material-based splashing)** — research-grade; a crown mesh + sprite is enough.
- **Physically-based rain audio synthesis (ACM TOG material sound textures)** — research-grade; layered loops + intensity RTPC + occlusion filter is enough.
- **Volumetric fog for rain haze** — you don't have volumetrics; approximate with distance fog.
- **Half-res transparency / reduced-res compositing** — effectively post-process; defer until a compositing path exists.
- **Mesh/task shaders, GPU-driven append/consume, subgroup compaction** — overkill for stateless constant-count rain.
- **Making the visual particle sim deterministic** — actively harmful; wastes effort and buys nothing since drops are gameplay-irrelevant.

### Explicit "made harder/easier by" matrix

- **Determinism requirement:** EASIER for the whole thing than feared, *because* you decouple — visual particles escape it entirely; only a handful of CPU scalars must be deterministic. HARDER for exactly one thing: the CPU sky-exposure march through voxel SDF must use integer/fixed iteration counts (no float-tolerance loop termination).
- **AI-native text-first authoring:** EASIER — flat schema params (`intensity`, `wind`, `porosity`, `wet_darkening`, puddle-mask thresholds) are ideal for TOML + schemars + agent description. Node-graph techniques (Unity's subgraph approach) must be flattened into named scalar params. This actively pushes you toward the simpler, better architecture.
- **Destructible voxel terrain:** HARDER for depth-map occlusion (baked map invalidated) — which is *why* ray queries win; EASIER for CPU occlusion (you already have the SDF to march). Forces BLAS rebuilds not refits (the main perf watch-item).
- **No post-process stack:** HARDER — kills composite rain, lens drops, SSR, half-res compositing. This is the dominant constraint and shapes the entire v1.
- **Hardware ray queries:** EASIER — gives you true-3D occlusion, sky-exposure queries, and ray-traced puddle reflections (your SSR substitute), all reusing existing BVH infrastructure.

## Recommendations

**Dependency ordering (what must exist before what):**
1. CPU authoritative rain state (`intensity`, `wind`, `is_exposed` via voxel-SDF sky march) — this is the deterministic root everything keys off, and it's schema-authorable TOML. Build first.
2. Stateless GPU streak renderer (PCG-hash positions, camera-locked wrapping volume, additive blend). Depends on (1) for parameters.
3. Ray-query sky-occlusion compute pass producing an exposure value / splash-spawn list. Depends on your existing BVH.
4. Wetness material params in the forward shader (darken/roughness/porosity), driven by accumulated exposure from (1)+(3).
5. Splash + ripple decals/particles, spawned from (3). Ripple atlas first; height-field seeding of your water sim later.
6. Audio intensity crossfade + occlusion low-pass, keyed off (1)+(3).
7. (Later) ray-traced puddle reflections; drips/runoff; lightning; half-res compositing once a post-process path exists.

**Concrete minimal-but-convincing v1 (~3–5 developer-weeks with an AI agent):**
- Stateless camera-locked stretched-billboard streaks, additive blend, PCG positions, soft depth fade (~4–6 days).
- CPU deterministic rain state + voxel-SDF sky-exposure query, TOML-authorable, sim-asserted (~3–5 days).
- Wetness darkening/roughness in the forward material with accumulation/drying (~3–4 days).
- Splash particles + scrolling ripple-normal atlas on flat/exposed surfaces, count driven by intensity (~3–5 days).
- Audio: 2–3 intensity-crossfaded loops + occlusion low-pass reusing the exposure fraction (~2–3 days).

**Staged path to impressive (v2+):**
- Ray-query occlusion replacing/augmenting the SDF march for crisp per-splash placement and true overhang handling (~3–5 days).
- Puddle placement from terrain flow-accumulation + curvature, feeding the wet mask and the height-field ripple sim seeding (~1 week).
- Ray-traced reflections on hero puddles (~3–5 days).
- Lighting-volume drop illumination for headlights/lamps; drips/runoff; lightning (~1 week total).
- Half-res rain compositing once a post-process stack lands.

**Benchmarks/thresholds that change the plan:**
- If GPU frame time spikes under heavy rain, the cause is overdraw — first switch to additive/premultiplied and trivial shader, then cap the near-volume count before anything else.
- If BLAS rebuild for voxel destruction already exceeds ~2 ms/frame (NVIDIA's stated budget), do NOT add more ray-query load in the same frame; distribute rain occlusion rays across frames or fall back to the SDF march for occlusion.
- If the AI agent's PNG offscreen renders show rain but sim asserts are flaky, you've accidentally coupled visual particles into the sim — re-audit that only the CPU scalar state feeds the hash.

## Caveats

- **No public rays/second figure exists for the RTX 4090** (NVIDIA confirmed: no published gigarays since Turing); all ray-cost numbers here are from a 3080-class Tellusim benchmark (0.55 ms for ~4.3M rays at 900p; 7 ms/18 ms BLAS rebuild for 490K tris) scaled by RT-core count, and must be verified with Nsight on your hardware. Treat the "ray tracing is cheap, BVH is the cost" conclusion as directionally certain but numerically to-be-profiled.
- **You are tested on exactly one configuration (RTX 4090 / NVIDIA / Fedora).** Alpha-to-coverage, MSAA interaction, additive-blend precision, and ray-query occupancy can differ on other vendors/drivers; none of this is validated off your single box.
- **The "no golden-image regression test" gap will bite rain specifically** — rain looks fine in a screenshot and wrong in motion (this is the single most common rain failure mode; the perceptual literature is explicit that rain is a *temporal* phenomenon). Your PNG-offscreen verification cannot catch motion artifacts (frozen-relative-to-world drops, seam popping when the volume re-anchors, streak orientation errors that only show when the camera rotates). Budget manual motion review, or build a short deterministic camera-fly-through that dumps a frame sequence.
- **Determinism silent-break risks:** (a) the visual GPU rain must never write anything that feeds `loom sim` state; (b) the CPU sky-exposure march through voxel SDF must use integer/fixed iteration counts, not time- or float-tolerance-based loop termination that could diverge across platforms; (c) if you port PCG to both Rust and Slang, a mismatch (e.g., `u32` overflow/wrapping semantics, different multiply constants) silently desyncs CPU gameplay from GPU visuals — unit-test that the Rust and Slang PCG produce identical outputs for a fixed input set.
- **Puddle-from-flow-accumulation trap:** flow accumulation marks large drainage convergence, not necessarily flat puddle basins; without a slope/curvature gate you'll get water "climbing" gentle slopes. Use your existing slope/walkability analysis as the gate.
- **Industry genuinely disagrees / unsolved:** there is no consensus "correct" real-time rain — the field splits between particle-based and texture/composite-based, and every shipping game hand-tunes. Physically-based falling-rain appearance (Garg & Nayar) remains offline-only in practice; no shipping game uses the full streak database in real time. Ray-traced per-drop occlusion for rain is, as far as public sources show, unprecedented in shipping games (everyone uses the depth map) — you'd be doing something novel, justified only by your pre-existing BVH and destructible terrain.