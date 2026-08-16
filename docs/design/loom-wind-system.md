# Realistic and Performant Wind for the Loom Engine: A Cross-Cutting Design

## TL;DR
- **Build one deterministic analytic wind field in Rust as the single source of truth** — a base directional vector plus a small number of sinusoidal gusts plus one octave of curl noise — sampled identically by CPU gameplay, rapier physics, and (via a Rust→Slang codegen mirror) GPU vertex shaders. This mirrors the analytic-Gerstner-waves pattern Loom already chose for water and is the only architecture that survives the determinism and AI-authoring constraints intact.
- **The single highest-visibility payoff is GPU vertex-shader vegetation wind** (Tiago Sousa's Crytek GPU Gems 3 main-bend/detail-bend model plus Ghost of Tsushima–style compute-generated grass), which never touches the sim hash and so is "free" with respect to determinism; the hard physics (cloth, drag) should be a small, deterministic, CPU Verlet/XPBD system inside the fixed step.
- **The dominant risks are motion artifacts invisible to a still PNG** — unison sway, "swimming" vegetation, instant wind-direction snaps, grass shimmer, and silent CPU/GPU field divergence. Loom's only visual verification is a static screenshot, so these are a structural blind spot and must be guarded by asserting on the analytic field's numeric outputs, not on pixels.

## Key Findings

1. **The architecture question is already answered by Loom's own precedents.** Water (analytic Gerstner on CPU, Slang generated from Rust) and rain (CPU-authoritative scalar+wind-vector state in the hash, stateless GPU particles outside it) establish the exact pattern wind should follow. Wind is simply the shared upstream field both already want. Do not invent a second wind abstraction.

2. **Curl noise (Robert Bridson, Jim Hourihan, Marcus Nordenstam, "Curl-Noise for Procedural Fluid Flow," ACM ToG 26(3) Art. 46, SIGGRAPH 2007) is the right turbulence primitive but you likely only need a cheap subset.** It gives divergence-free ("incompressible-looking") swirl by taking the curl of a noise potential — described in the paper as "an extremely simple approach to efficiently generating turbulent velocity fields based on Perlin noise, with a formula that is exactly incompressible"; it is a handful of noise evaluations, trivially deterministic if the noise is deterministic. For wind you rarely need true 3D incompressibility — Ghost of Tsushima shipped with "just a vector + time-varying Perlin noise + vorticles."

3. **Vegetation wind is a solved, cheap, vertex-shader problem** with two canonical references: Tiago Sousa's Crytek GPU Gems 3 Ch.16 (main bending + detail bending, vertex-color-encoded stiffness/phase) and Sucker Punch's compute-generated per-blade grass. Both are GPU-only and therefore orthogonal to determinism.

4. **Deterministic cloth is a small CPU Verlet/XPBD system, not rapier.** rapier3d has no soft-body/cloth support (it "simulates the physics of rigid bodies"). Flags/capes/banners are the cheap, high-impact win. The Rust ecosystem's cloth option (bevy_silk) is Bevy-coupled and not production-grade — you will write your own fixed-step Verlet solver.

5. **Wind-as-physics-force is mostly not worth it** beyond a handful of gameplay-relevant bodies (light debris, the player's sailing/gliding, projectiles). The drag equation is trivial; the trap is determinism of force accumulation order.

6. **Wind-driven waves connect to water via ocean spectra** (Phillips / Pierson-Moskowitz / JONSWAP). You can derive a plausible Gerstner set from a single wind-speed scalar rather than hand-authoring amplitudes, and you must lag the wave field behind wind-direction changes ("wave inertia") or a rotating ocean looks wrong.

7. **Audio is a natural fit for Loom's ray-traced acoustics.** Layered filtered-noise synthesis crossfaded by wind speed, with the same sheltering query that drives visual occlusion feeding interior/exterior mix — a genuine, defensible advantage of the existing acoustics system.

## Details

### 1. Wind field representation — the core architecture

**Recommended model (the single source of truth).** Define, in a `loom_wind` crate, a pure function:

```
fn wind_at(pos: Vec3, t: f64) -> Vec3
```

composed of layered terms, all driven by flat, schemars-validated scalar parameters in the `.loom` TOML:

- **Base directional wind:** a horizontal direction (azimuth) + base speed. Author these on a Beaufort-like scale (0 calm … 12 hurricane) so the agent has intuitive, describable knobs. The Beaufort scale maps named states ("fresh breeze", "gale") to m/s bands and is the natural authorable parameterization.
- **Gusts:** a sum of 2–4 sinusoids in time (and slowly in space), each with amplitude, frequency, and phase. This is the cheapest possible "variation over time" and is exactly what Crytek's `SmoothTriangleWave` and SpeedTree's "gust" parameter do. Gusts should modulate the *magnitude* of the base vector, not replace it.
- **Turbulence:** one to two octaves of curl noise (see below) added as a spatially-varying vector, scaled down relative to base speed.
- **Optional vertical profile:** scale horizontal speed by a boundary-layer wind profile so speed increases with height.

**Curl noise, concretely.** Take a vector potential Ψ = (ψ₁, ψ₂, ψ₃) where each component is an independent 3D noise field, then the wind vector is v = ∇×Ψ, computed by finite differences: sample each noise component at small ± offsets along each axis and combine the partial derivatives. Because the curl of any field is divergence-free by construction, particles neither bunch up nor thin out — the "incompressible look." Cost is roughly 6 noise evaluations per curl sample (2 per axis via central differences), or fewer with the two-scalar cross-product variant (∇a × ∇b, which is divergence-free because the divergence of the cross product of two gradient fields is zero). Boundaries/obstacles are handled by modulating the potential to zero near solid surfaces (ramp Ψ down within a distance of a surface), which is exactly where Loom's **voxel SDF** shines: the SDF already gives a cheap signed distance to geometry, so you can taper the potential using the SDF value directly. **Is it worth it?** For a wind *field* driving vegetation and particles, plain 3D value/Perlin noise is usually visually sufficient and cheaper; curl noise's incompressibility matters most for closed-loop particle advection where you don't want sinks. Recommendation: ship v1 with plain fBm turbulence, add curl noise only if particle advection shows visible sink/source artifacts. (Note: SIGGRAPH Asia 2025's "Improving Curl Noise" generalizes the construction to n-D and smoother boundary handling, but is research-grade overkill here.)

**Determinism.** The entire field must be evaluated with a deterministic noise implementation and no `f32` non-associativity surprises across debug/release. Use fixed iteration order in the octave sum, accumulate in a defined order, and choose one float width consistently. This is the same discipline Loom already applies to Gerstner waves.

**Rust→Slang mirror.** Follow the water precedent exactly: author `wind_at` once in Rust, and generate the Slang version from the Rust source (or from a shared spec) so the CPU and GPU evaluations cannot silently diverge. This is the single most important structural decision in the entire system (see traps).

**Volume textures / flow maps.** 3D wind textures (authored or streamed) are how Unreal/Unity provide spatially-varying wind. For Loom, a hand-authored 3D wind texture fights the AI-text-first authoring requirement (a binary blob is not diffable). Prefer the analytic function. The one place a texture earns its keep is the *performance bake* (§7): the CPU/compute writes a small scrolling 2D/3D lookup once per frame so vertex shaders do one fetch instead of many noise octaves — that texture is a derived cache, not authored content, so it doesn't violate the authoring model.

**Real Eulerian fluid sim (Stable Fluids / Stam, GPU Navier-Stokes).** Almost never justified in a game for *wind*, and here it is disqualified twice over: it is expensive, and a GPU fluid sim feeding gameplay would require GPU→CPU readback (explicitly disqualifying) or a CPU sim (too slow at useful resolution). Skip entirely.

**Terrain-driven wind.** Real effects — acceleration over ridges, sheltering in valleys, katabatic/anabatic slope flows, and the logarithmic/power-law boundary-layer profile (wind speed rising with height above ground) — are mostly cosmetic for gameplay. The **power-law profile** (v(z) = v_ref·(z/z_ref)^α, α≈1/7 ≈ 0.143 for neutral atmospheric stability, per the standard wind-profile power law) is a one-line, cheap, deterministic height scaling worth adding; the more physically-grounded **log wind profile** (Prandtl: v(z) = (u*/κ)·ln(z/z₀), von Kármán constant κ≈0.4) is the alternative but needs a roughness length z₀ and is fiddlier. Loom's `loom_terrain` already computes slope and flow accumulation, so a cheap "speed-up over ridges, shelter in hollows" term driven by terrain slope is feasible and on-brand, but it is a v2+ nicety. No mainstream shipping game does full terrain-CFD wind; they fake it.

**Wind occlusion / sheltering.** Two candidate queries are available: hardware ray queries (VK_KHR_ray_query, already used for sun shadows) and the voxel SDF raymarch (already used for rain's sky-exposure query). **Recommendation: use the voxel SDF for sheltering, not ray queries.** Reasons: (a) rain already established a voxel-SDF sky-exposure query in the deterministic sim — reuse it so the same shelter value drives visual weakening, physics, and audio; (b) SDF gives a smooth, cheap, distance-based falloff ideal for gradual sheltering, whereas a single ray query gives a binary hit; (c) it keeps the sheltering query CPU-side and deterministic. Ray queries are better only if you need precise, per-pixel, thin-geometry occlusion on the GPU for the visual layer.

### 2. Vegetation animation — the highest-visibility consumer

**Tiago Sousa / Crytek GPU Gems 3 Ch.16 (the canonical model).** Two levels: (1) **main bending** displaces the whole plant's xy along the wind direction, scaled by normalized height (so the base stays planted, the top moves most), with a re-normalization trick (`vPos.xyz = normalize(vNewPos.xyz)*fLength`) to constrain movement to a sphere and avoid stretching; (2) **detail bending** animates leaves/edges using vertex colors: red = edge stiffness, green = per-leaf phase, blue = leaf stiffness, alpha = precomputed AO. Waves are cheap `SmoothTriangleWave` approximations of sines (`SmoothCurve(TriangleWave(x))`), and phases are derived from object and vertex position (`fObjPhase = dot(worldPos.xyz, 1)`) so each plant and each leaf moves out of phase. The paper's own framing: "we divide animation into two parts: (1) the main bending, which animates the entire vegetation along the wind direction; and (2) the detail bending, which animates the leaves. A wind vector is computed per-instance, in world space." Crucially, this keeps "the per-vertex cost constant" regardless of the number of wind sources (they are summed into a per-instance wind vector on the CPU). This is the single most important vegetation reference and maps directly onto Loom's flat-parameter model.

**SpeedTree (8/9/10).** Ships a hierarchical wind model — global (whole-tree sway) → branch motion (multiple levels) → leaf ripple/tumble/twitch → frond → rolling wind — implemented in the games SDK entirely in vertex shaders with **no bones** ("There are no bones or bone weights used in the simulation… every instance can have a unique wind signature"). Cost is documented incrementally: e.g. enabling rolling wind "can add 100 instructions to full-effect leaf wind vertex shaders and about 30 to full-effect branch wind shaders." Wind quality is exposed as discrete presets (Unity's `_WindQuality` range 0–5: None/Fastest/Fast/Better/Best/Palm). It integrates in Unreal (ST9: wind fully in the material/GPU, tuned via a Master Material) and Unity. Loom won't license SpeedTree, but its parameter taxonomy (global/branch/leaf tiers, per-instance signature) is the model to imitate — and its incremental-cost structure is the blueprint for a Loom "wind quality" LOD knob.

**Unreal Engine current state (5.6/5.7).** `SimpleGrassWind` and `WindDirectionalSource` feeding a Material Parameter Collection (MPC) that all foliage materials sample is exactly the "one global wind, many consumers" pattern Loom wants (a Blueprint writes the wind actor's normalized direction + strength into the MPC each tick). **Pivot Painter 2.0** bakes per-branch pivot positions and hierarchy into textures/UVs so the material can rotate each branch about its own pivot — a way to get hierarchical bending without a skeleton. The critical current caveat: **Nanite historically did not support World Position Offset**, so classic vertex-animated wind on Nanite foliage was broken/limited; support has been progressively added but remains buggy (WPO + Nanite foliage shadow issues reported in UE 5.6). This matters to Loom only as a warning: Loom is a forward renderer with normal meshes, so it sidesteps the entire Nanite-WPO problem — vertex-shader wind Just Works here.

**Unity.** WindZone + SpeedTree shaders + Shader Graph WPO; DOTS/compute grass for scale. Same conclusions as Unreal.

**Grass specifically.** The state of the art is per-blade GPU-generated grass: **Ghost of Tsushima** (Sucker Punch; grass talk by Eric Wohllaib, GDC 2021 Advanced Graphics Summit) "chose to render their fields by generating individual blades of grass on the GPU that could each have their own procedural appearance and animation." The pipeline is placement compute shader → blade list → finalize compute shader → indirect draw → vertex/pixel shader, with blades modeled as cubic Bézier curves, **high LOD 15 vertices / low LOD 7 vertices**, and per-blade data (position, facing, wind strength at position, per-blade hash, grass type, clump facing/color, height, width, tilt, bend). Wind is applied by sampling scrolling 2D Perlin noise at each blade's world position; clumping uses Voronoi cells; culling (distance/frustum/occlusion) happens in the compute stage. This exact pipeline is reproducible in Loom's raw Vulkan + compute + indirect-draw stack, and because the grass is generated fresh each frame from a deterministic seed + the analytic wind field, it never needs to enter the sim hash. Genshin/BOTW-style stylized grass and Decima (Horizon) vegetation use variants of the same idea. Compute-shader grass is the right target for Loom's v2.

**Avoiding "everything sways in unison."** The fix is phase offsetting: derive each instance's (and each vertex's) phase from its world position (Crytek's `fObjPhase = dot(worldPos,1)`) and/or a per-instance hash, so neighbors are decorrelated. Encode stiffness/mass in vertex colors or UVs (Crytek's exact trick). This is the difference between "a field of grass" and "a rubber sheet."

**Consistency with the analytic field.** The whole point of the single-source-of-truth field: the grass compute shader, the tree vertex shader, a flag, and the rain all sample the *same* `wind_at()`. Do NOT let each system invent its own noise — that is how a flag ends up pointing a different direction than the grass bends. Feed all of them the shared wind vector (as a push-constant/UBO base + the shared noise parameters). This is precisely why Ghost of Tsushima built one wind system so "everything in the world reacts appropriately as the wind changes direction," with cloth and ropes "using the same wind inputs as the foliage and particles."

**Correct normals under deformation.** When vertices move, the lighting normal should change too, or specular/shading looks flat and static. The standard estimate is to run the deformation on two neighbor offset points (tangent + bitangent directions) and reconstruct the normal from their cross product. Honest industry answer: **most games don't bother for grass** (they fake rounded normals for blades via `FRONT_FACING`/outward-tilted normals instead) and only sometimes bother for hero trees. For Loom, skip normal recompute in v1; it is a motion-quality issue invisible in a still PNG anyway.

**Interactive foliage (bend away from player/vehicles).** The standard technique is a top-down "interaction/trample map" render target: entities write displacement into a texture that the foliage vertex shader samples to push blades away, with a damped-spring restore so grass springs back naturally rather than snapping. Ghost of Tsushima did exactly this, "applying a damped wave to the strength of the displacement, which prevents the grass from snapping back to its rest position in a linear and unnatural fashion." Cost is one small RT and a texture fetch. **Determinism note:** if trample only affects visuals, keep it out of the sim hash. If gameplay depends on it (hiding in grass), the trample state must be a deterministic CPU grid in the hash.

### 3. Cloth, flags, ropes, hair

**Deterministic cloth for a fixed step.** Options: Verlet mass-spring (simplest, position-based, very stable), PBD (Müller et al. 2007), and XPBD (Macklin et al. 2016). PBD's stiffness depends on iteration count and timestep — a determinism and tuning hazard ("given enough iterations it will converge to an infinitely stiff solution… stiffness does not have a physical basis in PBD"). **XPBD** fixes this by introducing a compliance parameter (α̃ = α/Δt², inverse stiffness) so material stiffness is decoupled from iteration count and timestep, which is exactly what you want when the fixed-step sim must produce identical results in debug and release (α=0 gives a rigid constraint; α typically 10⁻¹⁰–10⁻² for real materials). **Recommendation: Verlet for flags/ropes v1, XPBD if you later want believable cloth with tuned stiffness.** All are deterministic if you fix constraint iteration order and accumulate consistently. Ghost of Tsushima's cloth is a good existence proof of the cheap approach: GPU Verlet, 512 threads/group, up to 1152 "joints" per cloth, "gravity, wind, inertia, damping," explicitly *not* doing full matrix inversion ("To do it 'right' takes full matrix inversion — We're not doing that").

**rapier3d cloth?** No. rapier "simulates the physics of rigid bodies" — no soft-body/cloth/fluid support; it is rigid bodies + joints. Don't wait for it. Loom's own `loom_particles` deterministic CPU sim is a better foundation — a flag is a small constrained particle grid.

**Rust ecosystem.** `bevy_silk` is a CPU Verlet cloth engine for Bevy (flags/capes via `rectangle_mesh`), but it is Bevy-ECS-coupled, marks its rapier collision support "experimental… not suited for production," and explicitly smooths wind/gravity by framerate ("if the framerate drops suddenly gravity and wind get much stronger") — a determinism red flag. `bevy_verlet` is a tiny (~331 SLoC) Verlet toy. Conclusion: **read them for reference, write your own** fixed-step Verlet/XPBD in a `loom_cloth` crate reusing `loom_particles` patterns.

**Flags/banners/capes — the cheap win.** A small vertex chain (a 1D strip of Verlet particles for a pennant, a 2D grid for a banner), a few constraint iterations, one pinned edge, wind sampled from the field, aerodynamic force per triangle. High visual impact, tiny cost, fully deterministic. This should be the *first* cloth thing you build.

**Aerodynamic force on cloth.** Standard model: for each triangle, compute relative wind (wind velocity − cloth velocity), take the component along the face normal, and apply a force proportional to area × (relative wind · normal), i.e. F ≈ ½·ρ·Cd·A·(v_rel·n̂)·n̂. This gives the flag its billow and flutter. Lift/drag can be split but the normal-projection drag term alone looks good. (This is the classic "animation aerodynamics" model, Wejchert & Haumann 1991.)

**Ropes/cables/hair.** Ropes/cables are just 1D Verlet chains — cheap and worth it for rigging, vines, tolling banners. Hair is a much larger effort (guide strands + interpolation + collision) and is **not worth it** for a solo dev without a skeletal-animation system; skip.

### 4. Particle advection and existing-system integration

**Deterministic CPU particles (`loom_particles`).** Advect by sampling `wind_at(pos,t)` and adding wind velocity × dt in the fixed step. Because both the particle sim and the wind field are deterministic, this stays in the hash cleanly. Use a fixed update order over particles.

**Stateless GPU rain (Jarzynski & Olano PCG).** Rain is a closed-form function of index+time, deliberately outside the hash. Wind integration is purely visual: the streak's *orientation* must match the wind vector (streaks lean downwind), and horizontal drift is added as a function of the same wind vector the CPU-authoritative rain state already stores. Because the CPU rain state already carries a wind vector in the hash, feed that exact vector to the GPU rain shader so the leaning streaks agree with everything else. Rain streak lean = normalize(fall_velocity + wind_horizontal).

**Wind-driven wave generation (the water link).** Rather than hand-authoring Gerstner amplitudes, derive a wave set from wind speed via an ocean spectrum:
- **Pierson-Moskowitz** (1964, J. Geophys. Res. 69: 5181–5190): fully-developed sea from a single input, wind speed U₁₉.₅ **at 19.5 m** (the anemometer height on the British weather ships used in the study — *not* 10 m; be careful with this, as much of the derived literature restates it in U₁₀). Gives significant wave height and peak wavenumber directly from wind speed — a clean one-parameter mapping from wind speed to sea state (e.g. k_p ≈ 0.66·g/U², H₁/₃ ≈ 0.24·U²/g in the U-referenced forms).
- **JONSWAP** (Hasselmann et al. 1973, Joint North Sea Wave Project): adds **fetch length** X and a peak-enhancement factor γ (mean value ≈3.3), giving a sharper, fetch-limited spectrum (ω_p = 22·(g²/(U₁₀·X))^{1/3}). γ=1 reduces JONSWAP exactly to Pierson-Moskowitz. This is the one to use if you want "wind that has only blown across a short bay makes smaller, choppier waves."
- **Phillips**: the equilibrium-range tail; the classic FFT-ocean spectrum (Tessendorf-style sims) but you're doing analytic Gerstner, so PM/JONSWAP-derived discrete components are the better fit.

Practical recipe: pick N Gerstner components (shipping oceans commonly use ~6–16; e.g. *Asgard's Wrath* used 16 Gerstner waves in four directional clusters), distribute their wavelengths around the spectral peak k_p(U), set each amplitude from the spectrum value at that wavelength, and spread their directions around the wind direction with a **directional spreading** function (cos^{2s} of the angle off-wind). This lets a single wind-speed scalar drive the whole ocean, which is perfect for AI authoring.

**Wave inertia (do not skip).** Instantly rotating wave directions when wind direction changes looks wrong — real seas lag. Slew the wave-set direction toward the wind direction over tens of seconds, and let old swell persist while new wind-sea builds. Since Gerstner waves are the CPU source of truth, this is just rate-limiting the direction parameter each tick. This is a classic "looks fine in a screenshot, wrong in motion" trap.

**Dust, leaves, snow, embers, smoke.** All advect by the same `wind_at()` sampling. Bias: lighter particles (embers, leaves) get more wind influence (lower terminal velocity, higher drag response); heavy particles (rain, gravel) less. **Snow accumulation direction:** bias deposition toward the downwind side of obstacles using the wind vector and the SDF (drifts pile leeward). Smoke should advect through the curl-noise turbulence term specifically — this is where curl noise's incompressibility earns its keep (no visible sinks in a smoke column). Ghost of Tsushima's "vorticles" (vortex particles feeding leaf/smoke compute shaders) are the shippable version of localized swirl.

**Fire/smoke propagation.** Wind biasing fire spread is a gameplay-simulation decision; if you do it, it must be a deterministic CPU cellular update in the hash. Given Loom's destructible voxels, wind-biased fire is a plausible future feature but is out of scope for a wind v1.

### 5. Wind as a physics force

**Drag on rapier bodies.** Standard quadratic drag: F = ½·ρ·Cd·A·|v_rel|·v_rel, where v_rel = wind_velocity − body_velocity (this is the standard hydrodynamic/aerodynamic drag equation; Cd ≈ 1.05 for a cube, ~0.04 for a streamlined body, air density ρ ≈ 1.225 kg/m³). Approximate cross-sectional area A cheaply from the body's AABB face or a per-body authored scalar (don't compute true projected area). **Determinism discipline is the whole game here:** iterate bodies in a stable, index-sorted order; accumulate all wind forces then apply once per body per step (accumulate-then-apply); never let force order depend on HashMap iteration (already banned by clippy.toml). Apply as an external force before rapier's step. rapier's `enhanced-determinism` gives "cross-platform determinism (assuming the rest of your code is also deterministic) across all 32-bit and 64-bit platforms that implement the IEEE 754-2008 floating point standard" — the "rest of your code" clause is where sloppy force-application order will bite you.

**Projectiles (arrows/bullets).** Adding v_rel drag + crosswind to a projectile is cheap and gameplay-meaningful (archery). Worth it if the game has ranged combat; otherwise cosmetic.

**Player-facing effects.** Being pushed while walking (add wind force to the character controller), gliding/parachutes (large A, wind dominates), and **sailing** (see below). These are the wind forces that players actually *feel*, so they're worth more than making every crate rattle.

**Sailing.** Loom now has water + buoyancy, so sailing is the natural showcase. Published game-dev material is thinner than for vegetation, but the physics is textbook: sail force decomposes into lift (perpendicular to apparent wind) and drag (along it); apparent wind = true wind − boat velocity; the boat can sail faster than downwind on a beam reach because of lift. A believable arcade model: compute apparent wind from `wind_at()` minus hull velocity, project onto sail normal for thrust, apply keel lateral resistance, and let rapier integrate. This is a strong, differentiating v3 feature given the water/buoyancy investment.

**What's worth building vs. visual-only.** Worth building as real forces: player push, gliding/sailing, projectiles (if ranged combat), and a *small* number of light dynamic props. Everything else — trees, grass, distant debris — should be **visual-only** wind in the vertex shader. Do not run drag on thousands of bodies; it is neither performant nor necessary.

### 6. Audio

**Standard implementation.** Wind audio is normally layered looping samples crossfaded by wind speed (calm rustle → moderate → howl), often with filtered-noise synthesis (brown/white noise through a speed-controlled low-pass, gain and filter driven by wind speed — the approach used in shipping vehicle/weather systems built on FMOD/Wwise). Procedural filtered-noise synthesis beats loops for wind specifically because wind is broadband and non-periodic — loops audibly repeat, synthesis doesn't ("No recorded loops = no audible repetition"). Rockstar's RAGE (GTA V, ~30% procedural audio assets) and FMOD/Wwise procedural weather plugins (e.g. AudioGaming's AudioWeather/AudioWind) synthesize wind this way.

**Aeolian tones.** Wind over edges/wires/gaps produces tonal whistling from vortex shedding (the Aeolian tone), whose frequency scales with wind speed and inversely with the obstacle's size (Strouhal relation). A few systems synthesize these in real time; Dobashi et al. precomputed aeolian/cavity tones from CFD and modulated them at runtime by wind speed ("an airspeed model that replicates the wind can reproduce the sound of wind through a fence"). This is a nice-to-have, not a v1.

**Loom's specific advantage.** `loom_audio` ray-traces real scene geometry for acoustics. This is a genuine, unusual advantage for wind: (a) use the **same voxel-SDF sheltering value** that drives visual/physics sheltering to crossfade interior/exterior wind mix and to attenuate the howl when sheltered — one query, three consumers; (b) the ray-traced acoustics can derive where wind "finds" gaps/edges (caves, doorways) and place howl/whistle sources there physically, rather than hand-placing them. This is a defensible, on-brand feature that most engines can't do.

**Vegetation rustle.** Key rustle loudness/density to the aggregate bending amount of nearby foliage (which you already compute from `wind_at()`), so the leaves you *see* moving are the leaves you *hear*. Cheap and high-immersion.

### 7. Performance engineering

**Vegetation wind cost is small and well-characterized in instructions.** The Crytek technique re-implemented in UDK is **71–95 vertex-shader instructions** depending on settings, versus 130 for the UDK foliage-demo shader ("depending on the settings this shader uses 71 to 95 instructions, opposed to the 130 instructions used by the demo shader"); an independent re-implementation of the same GPU Gems 3 technique reports **~62 instruction slots**. Detail bending is a handful of `SmoothTriangleWave` evaluations. The per-vertex cost is constant regardless of the number of wind sources (Crytek folds them into a per-instance wind vector on the CPU). SpeedTree documents wind cost incrementally — each enabled effect tier adds instructions (rolling wind: +100 to leaf, +30 to branch vertex shaders) — which is the model for a Loom "wind quality" LOD knob. **Honest gap:** these are *instruction counts*, not measured microseconds; no primary source publishes a clean per-frame µs cost for the Crysis/SpeedTree wind vertex shader, so treat instruction count as the proxy.

**Noise-per-vertex vs. precomputed wind texture.** Evaluating multi-octave noise per vertex is meaningfully more expensive than a single texture fetch; a classic optimized 3D Perlin evaluation is on the order of **~50 shader-model-2.0 instructions per octave** (GPU Gems 2, "Implementing Improved Perlin Noise"), so N octaves ≈ N× that, versus one bilinear fetch for a baked field. **The standard optimization** is to bake the low-frequency wind field into a small scrolling 2D (or 3D) texture, updated once per frame on the CPU or in a tiny compute pass, so every vertex does one fetch instead of many noise octaves. Ghost of Tsushima's grass does exactly this (scrolling 2D Perlin sampled per blade). **Honest gap:** I found no published head-to-head "N octaves vs. one fetch, in ms" benchmark — the instruction-count argument is the defensible basis. **Is it the right call for Loom?** Yes for grass/dense foliage at scale (v2). For v1 with modest vegetation counts, per-vertex analytic evaluation is simpler and fine. The subtlety: **the baked texture must be generated from the same `wind_at()` used on the CPU**, or you reintroduce divergence.

**LOD for wind.** Reduce detail bending with distance (drop leaf flutter first, keep main bending), and drop wind entirely on far LODs. The trap is **popping** when a plant crosses the LOD boundary and its wind animation suddenly changes amplitude — cross-fade the wind amplitude over a distance band rather than switching hard.

**Vertex shader vs. compute prepass.** For sparse foliage, do wind in the vertex shader. For millions of grass blades, generate blades in a compute prepass with indirect draw (Ghost of Tsushima model). **Mesh/task shaders** are a viable modern path for grass on Loom's RTX 4090 target and worth prototyping in v2, but the compute + indirect-draw path is better documented and lower-risk. Instancing + indirect draws is mandatory for foliage regardless.

**Calm→storm without a frame-time cliff.** Cost must not scale with wind *strength* — a storm should execute the same shader instructions as a calm day, just with larger amplitudes. Avoid any "if windy, add more waves" branching that creates a frame-time cliff during storms (when you can least afford it). Keep the octave/tier count fixed; scale amplitudes.

**Vulkan-specific placement (Loom).** Put the global wind parameters (base vector, gust params, time, noise seeds) in **push constants** if small enough, or a per-frame uniform buffer; with bindless/BDA you can also point at a wind parameter buffer by device address. Per-instance wind signature (stiffness, phase seed) rides in the instance buffer, not recomputed on the CPU each frame. If you do the compute-prepass wind texture, weigh the `loom_render_graph` barrier cost: one compute dispatch + one barrier before the foliage pass is cheap and almost always worth it when foliage vertex count is high; not worth it for a few hundred plants.

### 8. Rust ecosystem assessment

- **`noise` (noise-rs, Razaekel):** the most complete pure-Rust noise library (Perlin, OpenSimplex, `Fbm`, worley, etc.), actively used. Deterministic given a fixed seed and version. **Caveat for Loom:** it is a *host* library; you still need a matching Slang implementation for the GPU, and you must pin the crate version so noise output doesn't shift under you (a silent-divergence risk). This is the strongest argument for generating the GPU noise from the same definition as the CPU.
- **`fastnoise-lite` (Rust port of FastNoiseLite):** "extremely portable… focuses on high performance while avoiding platform/language specific features, allowing for easy ports to as many possible languages" — which is a real advantage here because a matching HLSL/GLSL/Slang port is easier to keep bit-comparable across CPU and GPU. Good candidate precisely for the CPU/GPU-mirror requirement.
- **`bracket-noise`:** Rust port of Auburn's FastNoise, part of bracket-lib; fine, less active.
- **`simdnoise`:** SIMD-accelerated; fast but SIMD paths are a **determinism hazard** across debug/release and CPU targets (the exact thing `cargo xtask validate` will catch). Avoid for anything in the sim hash unless you verify bit-identical output.
- **Cloth:** `bevy_silk` (CPU Verlet, Bevy-coupled, collision "experimental… not suited for production", framerate-smoothed wind), `bevy_verlet` (~331 SLoC toy). Neither is drop-in for a deterministic fixed-step non-Bevy engine. **Write your own.** (Note also that `bevy_xpbd` has been deprecated in favor of its successor `avian` — another reason not to build on a moving target.)
- **Physics:** rapier3d with `enhanced-determinism` gives bit-level cross-platform determinism on IEEE-754-2008 platforms — Loom already depends on this. No soft bodies.
- **Vegetation-wind references worth reading:** `tuxalin/vegetation-shader` (UE4 procedural wind material implementing the GPU Gems 3 model with vertex-color/texture-encoded branch data, global wind via a material collection), the many open Ghost-of-Tsushima-style grass repos (Godot `2Retr0/GodotGrass`, Unity `cainrademan/Unity-Grass`, `harlan0103/Grass-Rendering-in-Modern-Game-Engine`), and IceFall Games / minifloppy write-ups of the Crytek shader.

### 9. What to skip (with reasons)

- **Real-time Navier-Stokes/Eulerian wind sim:** too expensive; needs readback or a slow CPU sim; determinism-hostile. The whole industry fakes wind — so should you.
- **GPU→CPU readback of any wind data feeding sim:** explicitly disqualifying in Loom (non-reproducible timing).
- **Hair simulation:** needs infrastructure (skinning, guide strands) Loom lacks; not worth solo-dev time.
- **Normal recomputation under wind deformation:** motion-quality nicety, invisible in a still PNG, skip in v1.
- **Full terrain-CFD wind (ridge acceleration by simulation):** cosmetic; approximate with a cheap slope/height term if at all.
- **Cloth via rapier / waiting for a Rust soft-body crate:** doesn't exist; build a small Verlet/XPBD instead.
- **Volumetric wind/god-ray interaction, motion-blur streaking:** needs the post-process stack and motion vectors Loom doesn't have. Skip until those exist.
- **Curl noise in v1:** start with plain fBm; add curl only where particle sinks are visible.

### Dependency ordering

1. **`loom_wind` analytic field in Rust** (base + gusts + optional noise), schemars-validated TOML params, in the deterministic sim, with sim-hash assertions on sampled values. *Everything depends on this.*
2. **Rust→Slang mirror + a wind UBO/push-constant** exposed to shaders. (Depends on 1.)
3. **Vegetation vertex-shader wind** (Crytek main+detail bend), sampling the shared field. (Depends on 2.)
4. **Particle advection** in `loom_particles` + rain-streak orientation. (Depends on 1–2.)
5. **Flags/ropes Verlet cloth** in a new `loom_cloth` crate reusing `loom_particles`. (Depends on 1.)
6. **Wind→physics drag** on a small body set + player push. (Depends on 1.)
7. **Wind→wave coupling** (spectrum-derived Gerstner + wave inertia) into the water system. (Depends on 1 and the water system.)
8. **Audio** layers + sheltering-driven mix. (Depends on 1 and the SDF sheltering query.)
9. **Sheltering** (voxel-SDF) feeding visual/physics/audio. (Depends on 1.)
10. **Perf: compute-prepass wind texture + compute grass.** (Depends on 3.)

### Effort estimates (one competent dev + AI coding agent)

- `loom_wind` field + determinism tests + TOML schema: **3–5 days.**
- Rust→Slang mirror + shader plumbing: **2–4 days** (codegen is the fiddly part).
- Vegetation vertex-shader wind (trees + simple grass): **4–7 days.**
- Particle advection + rain streak orientation: **2–3 days.**
- `loom_cloth` Verlet flags/ropes + aero force: **5–8 days.**
- Wind→physics drag + player push: **2–4 days.**
- Wind→wave spectrum coupling + inertia: **4–6 days** (plus water system existing).
- Audio layers + sheltering mix: **3–5 days.**
- Voxel-SDF sheltering query (shared): **2–4 days.**
- Perf pass (baked wind texture, compute grass): **1–3 weeks** for full compute-grass; **3–4 days** for just the baked texture.

**Minimal-but-convincing v1 (~2–3 weeks):** analytic `loom_wind` field (base + gusts + one noise octave) in the sim with hash assertions; Rust→Slang mirror; Crytek vertex-shader wind on trees and simple instanced grass; particle advection + leaning rain; one waving flag (Verlet). This is demonstrably "realistic wind" and everything agrees on direction.

**Impressive v2–v3 (add 1–2 months):** compute-generated per-blade grass with clumping and a trample/interaction map; spectrum-derived wind-driven waves with inertia; sailing on the buoyancy system; sheltering-driven audio with aeolian howls; curl-noise turbulence for smoke/dust.

## Recommendations

1. **Build `loom_wind` first, as a pure deterministic Rust function, and put its sampled outputs in the sim hash.** Write `loom sim` assertions on `wind_at()` at fixed positions/times *before* any visuals exist. This makes the field itself testable despite the PNG-only visual verification blind spot. Threshold to proceed: debug/release hashes agree on wind samples across 10k ticks.
2. **Mirror Rust→Slang via codegen, never by hand.** Add a build-time check that samples the field at a grid of points on both CPU and GPU (offscreen render encoding wind vectors as color, read back *only in a test harness, never in sim*) and asserts they match within a tight epsilon. This is the one automated defense against the highest-severity failure mode (silent CPU/GPU divergence). Threshold: max per-channel difference < 1/255 over the test grid.
3. **Ship vegetation wind (Crytek model) as the visible v1 payoff.** It's cheap (~62–95 vertex-shader instructions), GPU-only, determinism-free, and directly authorable with scalars + vertex colors — perfectly matched to Loom's constraints.
4. **Use the voxel SDF (not ray queries) as the single sheltering query** feeding visuals, physics, and audio. One deterministic query, three consumers.
5. **Do cloth as your own small fixed-step Verlet/XPBD in `loom_cloth`; start with flags.** Ignore rapier soft-body and bevy_silk except as reference.
6. **Keep hard wind forces to bodies players feel** (push, gliding, sailing, projectiles). Everything else is vertex-shader visual wind.
7. **Derive waves from a wind-speed scalar via PM/JONSWAP and lag direction changes.** This is the cleanest AI-authorable ocean and avoids the "instantly rotating ocean" motion artifact.
8. **When you optimize, bake the field to a scrolling texture generated from the same `wind_at()`** and adopt compute-generated grass. Re-verify CPU/GPU agreement after baking.

**Benchmarks that change the plan:** if per-vertex noise shows up as a vertex-shader bottleneck in profiling → move to the baked wind texture earlier. If particle advection shows visible sinks/clumping → switch that turbulence term to curl noise. If storms cause a frame-time cliff → audit for strength-dependent branching and make the shader cost constant.

## Caveats and traps (the highest-value section)

**Motion artifacts invisible to a still PNG (structural blind spot).** Loom verifies visuals only with a static screenshot, so an entire class of wind bugs is undetectable by the agent's own tooling:
- **Unison sway** ("the whole field is a rubber sheet") — looks fine frozen, obviously wrong in motion. Guard by asserting phase decorrelation numerically (e.g. assert neighboring instances have different phase seeds), not visually.
- **"Swimming"/"crawling" vegetation** — when the wind scroll speed or phase gradient is wrong, blades appear to slide across the ground rather than sway in place. Invisible in a still.
- **Instant wind-direction snaps** — direction changes must be slewed/rate-limited; a hard snap is jarring only in motion.
- **Grass shimmer/aliasing** — sub-pixel blades twinkling under wind; a temporal artifact a still won't show, and Loom has no TAA to hide it.
- **Wave direction snapping** — the ocean-inertia trap above.
- **Mitigation:** since you can't diff motion, encode the motion-correctness invariants as numeric assertions on the analytic field (phase continuity, bounded direction slew rate per tick, decorrelated phases) and check *those* in `loom sim`.

**Silent CPU/GPU field divergence — the highest-severity trap.** If the Rust `wind_at()` and the Slang `wind_at()` disagree even slightly (different noise implementation, different float rounding, different octave summation order, different `fract()`/hash constants), then physics/gameplay (CPU) and the visuals (GPU) will slowly desynchronize — a flag will bend one way while the drag pushes another, and *nothing will flag it* because each side is internally consistent. This is why the water design generates Slang from Rust, and wind must too. **Never** write the two implementations independently.

**Determinism traps specific to wind:**
- **Noise crate version drift:** upgrading `noise`/`fastnoise-lite` can change output and silently alter every hash. Pin the version; treat noise output as part of your ABI.
- **Force accumulation order:** applying wind drag to rapier bodies in HashMap-iteration order is non-deterministic (already banned by clippy.toml, but the danger recurs anywhere you iterate a set). Sort by stable index; accumulate-then-apply.
- **`f32` non-associativity across debug/release:** the octave sum a+b+c+d can round differently if reordered by the optimizer. Keep the summation order fixed and simple; this is exactly what `cargo xtask validate` exists to catch.
- **SIMD noise (`simdnoise`):** can produce different results across builds/targets. Keep out of the hash.
- **Time source:** wind is a function of sim time (the fixed-step tick count × dt), never wall-clock (`Instant::now` is banned) — good, but make sure the shader gets the *sim* time, not a GPU frame timer, or GPU wind will drift from CPU wind.

**Things that look correct but are subtly wrong:**
- **Main bending without re-normalization** stretches the mesh (Crytek's normalize-to-length step exists precisely to prevent this). Skipping it looks fine at low wind, tears at high wind.
- **Applying wind in the wrong space:** Crytek does everything in world space to avoid instancing discontinuities. Doing wind in object space makes rotated instances bend inconsistently.
- **Sampling the field at the mesh origin vs. per-vertex:** for a large tree, sampling wind only at the trunk base makes the whole canopy move rigidly; but sampling full turbulence per-vertex is expensive and can look jittery. Crytek's answer (per-instance wind vector + per-vertex phase from position) is the tuned middle ground.
- **Directional spreading sign errors** in waves make crests come from the wrong quadrant — subtle in a screenshot, wrong when you watch swell roll in.
- **PM spectrum wind-height confusion:** Pierson-Moskowitz is defined at 19.5 m, JONSWAP/most modern work at 10 m — mixing the reference heights silently mis-sizes your sea state.

**Things that only appear at scale:**
- **Phase discontinuities at chunk/instance boundaries:** if phase is derived from a coordinate that resets per voxel chunk or per instancing batch, you get visible seams where wind phase jumps. Derive phase from *global* world position, not chunk-local coords. This is aggravated by Loom's destructible voxel chunks — when a chunk regenerates, ensure the wind phase basis is stable across the chunk boundary.
- **Grass shimmer and overdraw** only bite with millions of blades — hence the compute + LOD approach for scale.
- **Storm frame-time cliff:** cost that scales with wind strength only hurts during storms.

**Destructible-voxel-specific:** when terrain is destroyed at runtime, the SDF sheltering query result changes — good (wind now reaches a newly-opened cave), but make sure the sheltering query re-samples the *current* SDF each tick deterministically, and that vegetation on destroyed voxels stops sampling wind (or you get floating swaying grass over a hole).

**Made harder/easier by each constraint:**
- **Determinism (a):** makes the *field* and *cloth/drag/particles* harder (must be bit-reproducible, careful float order, no readback) but makes *vegetation visuals* no harder (GPU-only, outside hash). Net: pushes almost everything wind-authoritative onto the CPU analytic field.
- **AI text-first authoring (b):** *easier* for flat scalar params (Beaufort speed, direction, gust amplitude) — the analytic model is ideal; *harder* for anything wanting a 3D texture/node-graph (curl-noise volumes, hand-painted wind maps) — avoid those.
- **Destructible voxels (c):** *harder* for phase continuity and sheltering (both must survive chunk regeneration) but *easier* for sheltering queries in general (the SDF is already there and already used by rain).
- **No post-process stack (d):** *removes* volumetric wind, motion-blur streaks, and TAA-hidden shimmer as options — simplifies scope but also removes the usual crutch that hides grass shimmer, so aliasing must be controlled at the source.
- **No skeletal animation/cloth yet (e):** *harder* for hair and skinned-character cloth (no foundation) but *irrelevant* to vegetation (vertex-shader, no skeleton — this is why SpeedTree uses no bones) and to flag Verlet (self-contained).
- **Hardware ray queries (f):** *available* as a sheltering option, but the voxel SDF is the better choice here for determinism and smoothness; ray queries are a fallback for precise GPU-only visual occlusion.
- **CPU-authoritative-analytic-field pattern already established (g):** the single biggest *easier* — the hardest architectural decision is already made and proven by water and rain; wind is a straightforward extension of a pattern the codebase already trusts.

**Where the industry has shifted (2025–2026):** the Nanite + World Position Offset foliage-wind story in Unreal is still not fully clean (WPO/Nanite shadow bugs reported in UE 5.6), so any assumption that "modern engines just do Nanite wind foliage" is stale — vertex-shader wind on ordinary meshes (Loom's situation) remains the robust path. Per-blade compute-generated grass (Ghost of Tsushima, 2021) has become the de-facto high-end grass standard rather than grass cards, and is directly reproducible in Loom's compute+indirect-draw Vulkan stack. XPBD (2016) has largely superseded plain PBD where tunable, timestep-independent stiffness matters — which is exactly the deterministic-fixed-step case — while the Rust XPBD crate landscape is itself in flux (`bevy_xpbd` deprecated for `avian`), reinforcing the "write your own small solver" recommendation.