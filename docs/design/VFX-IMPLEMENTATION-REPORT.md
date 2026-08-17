# Fire, smoke, and water VFX — the implementation-grade report

*Supplied by the human, 16 Aug 2026. Recorded verbatim.*

**Read `NIAGARA-AND-FIRE-RESEARCH.md` first, then this.** They are not the same document
and neither supersedes the other:

- **`NIAGARA-AND-FIRE-RESEARCH.md`** is Loom-specific and argumentative. Its §3 is four
  changes to one function that it argues carries most of the win, with file:line references
  into this tree. It answers *what is worth building here*.
- **This document** is engine-agnostic and mathematical. It carries the equations, the
  spectra, the solver pipelines, the Vulkan concerns and the shader pseudocode. It answers
  *how to build it correctly*.

Where they disagree on priority, the Loom-specific one wins, because it read this codebase.
Where they disagree on maths, this one wins.

**Scope note.** This describes work on the **engine** roadmap (P3 water, and a fire rework),
not the **editor** roadmap (`editor/PLAN.md`'s Stages 0–12). The two are independent tracks
and neither blocks the other.

---

## TL;DR

- Unreal's water, fire, and smoke are three separate technology stacks: (1) **Niagara**, a
  general GPU compute-particle framework whose "Simulation Stages" turn it into a
  grid-solver host; (2) the **UE5 Water plugin**, a spline-authored, Gerstner-wave surface
  with quadtree LOD tessellation and the Single Layer Water shading model; and (3)
  **Niagara Fluids**, grid Navier–Stokes (gas) and FLIP/PIC + shallow-water (liquid) solvers
  built on top of Niagara. All three are reproducible from published papers and can be built
  in Rust/Vulkan without touching UE source.
- For Loom, the highest-leverage, lowest-risk path to "Unreal-comparable" water is
  **Gerstner sum + FFT ocean (Tessendorf) + a camera-following interactive ripple
  render-target (2D wave equation) + buoyancy sampling that reads back the height field +
  Jacobian-driven foam and splash particles.** This is the architecture shipped by Sea of
  Thieves and by the Crest Ocean System — both far better documented than Epic's internals.
- Fire/smoke should be staged: first a **flipbook + six-way-lighting sprite path**, then a
  **3D grid gas solver (Stable Fluids + MacCormack advection + Jacobi pressure + vorticity
  confinement + buoyancy)** rendered by **ray-marching a 3D texture with Beer–Lambert +
  Henyey–Greenstein**. Blackbody radiation maps temperature→color.

---

## Key findings

1. **Niagara is a compute framework, not a particle system.** Its power comes from
   **Simulation Stages** (custom compute passes with iteration counts) and **Data
   Interfaces** (Grid2D/Grid3D/Neighbor Grid, mesh, collision, curves, render targets). A
   grid solver in Niagara *is* a set of simulation stages iterating over a Grid Collection
   DI. Loom can replicate this with a generic "compute stage graph" abstraction over Vulkan
   compute dispatches.
2. **UE5 Water waves are Gerstner/trochoidal**, not FFT by default. FFT (Tessendorf) is the
   higher-fidelity route used by Sea of Thieves. Both are height-field techniques; the Water
   plugin's distinguishing engineering is the **quadtree water-mesh LOD** and the **Water
   Info Texture / Water Zone** that bakes per-body height/flow/depth for sampling.
3. **Interactive water** in shipped UE titles is a **2D GPU wave-equation simulation in a
   render target**. Sea of Thieves used a GPU water surface simulation based on **Mei,
   Decaudin & Hu, "Fast hydraulic erosion simulation and visualization on GPU," PG'07**,
   ping-ponged each frame, with objects injecting displacement and the domain following the
   camera.
4. **Foam/whitecaps are driven by the Jacobian of the horizontal displacement field**: where
   `det(J) < threshold` the surface folds → foam. This one equation is used by Crest, Sea of
   Thieves, and virtually every FFT ocean.
5. **Fire color is blackbody radiation** (Planck's law) mapped temperature→RGB; smoke is a
   light-absorbing medium (Beer–Lambert). Both share the same simulated fields. Volumetric
   rendering is ray-marching with a Henyey–Greenstein phase function. **Six-way lighting** is
   the cheap production standard for flipbooks.
6. **The most implementable published designs to copy are Crest** (Unity, cascaded LOD data
   textures) **for water and GPU Gems 3 Ch.30 for 3D fluids** — both give near-complete
   algorithms.

---

## Part 1 — Niagara architecture

### 1.1 Object model and namespaces

- **System** — top-level effect. Owns System Spawn/Update scripts (once per system per frame,
  CPU) plus a list of Emitters.
- **Emitter** — one simulation with its own particle buffer, renderers, stages. Has Emitter
  Spawn/Update scripts.
- **Modules** — stackable behaviour units compiled into the stage scripts. They read/write
  **Attributes**.
- **Particle Attributes** — per-particle data (Position, Velocity, Color, Age, Lifetime, plus
  custom), stored **structure-of-arrays**.

Parameter namespaces: `Engine.*` (DeltaTime, sim time, owner position), `System.*`,
`Emitter.*`, `Particle.*`, `User.*` (exposed to gameplay), and transient `Module.*` scratch.
Reads and writes across namespaces form a dataflow graph the compiler resolves into a linear
read/write schedule per script.

### 1.2 Execution model

Per frame: **System Spawn (first) → System Update → [per emitter] Emitter Spawn/Update →
Particle Spawn → Particle Update → Simulation Stages → Renderers.**

Spawn scripts initialise; Update scripts advance. The stack **compiles to VM bytecode** for
CPU sim, or **HLSL → compute shaders** for GPU sim, where the whole emitter update is one or
more compute dispatches.

### 1.3 CPU vs GPU sim paths

- **CPU sim**: particles in RAM, updated by a register VM. Good for low counts, CPU-side
  events, precise spawning.
- **GPU sim**: attributes in GPU storage buffers, updated by compute. Required for large
  counts and grid solvers. Needs **Fixed Bounds** (dynamic bounds require a readback).

GPU particle management: **persistent double-buffered particle buffers**; a **free list / ID
allocator** recycling dead indices; a **spawn info** structure telling the spawn dispatch how
many to create and where; and **GPU-driven counts + indirect draw/dispatch** — the sim writes
the live count into a small GPU buffer feeding `vkCmdDrawIndirect`/`vkCmdDispatchIndirect`,
avoiding a readback.

### 1.4 Simulation Stages

A Simulation Stage is an extra compute pass with an **Iteration Source** (Particles = one
thread per particle, or a Data Interface = one thread per grid cell), an **Iteration Count**
(run N times per frame — essential for iterative solvers like Jacobi pressure), and an
optional per-stage target (Grid2D/Grid3D, Render Target 2D).

Stages execute **in order**, so multi-pass algorithms (advect → divergence → N× pressure →
project) map directly onto a sequence of stages. A simulation stage is often described as "a
Particle Update that can iterate within a single frame."

### 1.5 Data Interfaces

- **Grid2D Collection** — 2D grid of N float channels backed by render targets. "A
  programmable texture… each pixel is a grid cell, and your Simulation Stage logic is a
  compute shader that runs on every pixel."
- **Grid3D Collection** — 3D voxel grid; the backbone of gas/liquid grid solvers.
- **RasterizationGrid3D** — used by Niagara Fluids to scatter/rasterize particles into a
  volume in parallel.
- **Neighbor Grid3D** — uniform spatial hash binning particles into cells for neighbour
  iteration (SPH, inter-particle collision), typically driven from a Custom HLSL module.
- **Static/Skeletal Mesh DI** — sample positions, normals, velocities, UVs; spawn on
  surfaces; read mesh distance fields.
- **Collision Query DI** — depth-buffer (GBuffer) and **distance-field** collision.
- **Curve DI**, **Texture Sample DI**, **Render Target 2D DI**, **Array DIs**.

### 1.6 Renderers

**Sprite** (camera-facing/velocity-aligned), **Mesh** (instanced), **Ribbon** (trails,
connectivity by particle ID/age), **Light**, **Volume** (ray-marched from a Grid3D),
**Decal**, **Component**.

Sorting/culling: translucent sprites need back-to-front sort. GPU sim uses **GPU bitonic sort**
by per-view depth key, or accepts artifacts. Alternatives: OIT-style approaches, or dithered
masked opacity (cheaper, sorts against opaque depth).

### 1.7 Events

Emitters generate **events** (collision, death, location) into event buffers; another
emitter's **Event Handler** stage consumes them to spawn or modify particles. **Particle
Attribute Readers** let one emitter read another's attributes directly.

### 1.8 Collision math

**Depth-buffer collision** (screen-space): project to screen, read scene depth; if penetrating,
push out along the reconstructed normal. Cheap but **view-dependent** — Epic's own docs note
particles fall through geometry that leaves the view.

**Distance-field collision**: sample `φ(x)`. Colliding when `φ(x) < r`. The normal is the
normalised gradient, by central differences:

```
n̂ = ∇φ / ‖∇φ‖
∇φ ≈ (1/2ε)( φ(x+εx) − φ(x−εx),  φ(x+εy) − φ(x−εy),  φ(x+εz) − φ(x−εz) )
```

Response: push to the surface `x += (r − φ)n̂`, then reflect with restitution `e`:

```
v' = v − (1 + e)(v·n̂)n̂        then damp the tangential component by friction
```

Mesh SDF collision gives view-independent collision plus normal and velocity reads but scales
with mesh count; Global SDF is constant-cost but view-dependent and carries no mesh velocity.

**Analytical**: plane `(p, n)` penetration `d = (x−p)·n − r`; if `d < 0`, `x −= d·n` and
reflect. Sphere centre `c` radius `R`: colliding if `‖x−c‖ < R+r`, normal `(x−c)/‖x−c‖`.

---

## Part 2 — Water (highest priority)

### 2.1a Gerstner / trochoidal waves

A Gerstner wave moves surface points in circular orbits, producing sharp crests and flat
troughs. It is an exact rotational solution of the Euler equations (Gerstner, 1802). For
horizontal position `(x,z)`, unit direction `D`, wavenumber `k = 2π/λ`, amplitude `A`, angular
frequency `ω`, phase `φ`, steepness `Q`:

```
P.x = x + Σ Qi Ai Di.x cos(ki·x − ωi t + φi)
P.y =     Σ    Ai       sin(ki·x − ωi t + φi)
P.z = z + Σ Qi Ai Di.z cos(ki·x − ωi t + φi)
```

**Dispersion**: deep water `ω = sqrt(g k)`; finite depth `d`, `ω = sqrt(g k tanh(k d))`.

**Normal** (GPU Gems 1 Ch.1, Finch). With `WA = ωi Ai`, `S = sin(…)`, `C = cos(…)`:

```
B = ( 1 − Σ Qi Di.x² WA S,     Σ Di.x WA C,   −Σ Qi Di.x Di.z WA S )
T = ( −Σ Qi Di.x Di.z WA S,    Σ Di.z WA C,    1 − Σ Qi Di.z² WA S )
N = ( −Σ Di.x WA C,        1 − Σ Qi WA S,     −Σ Di.z WA C )
```

**Steepness / loop avoidance**: `Q = 0` is a rounded sine; `Q = 1/(ωA)` gives a sharp crest.
Summing waves, use `Qi = Q/(ωi Ai · numWaves)` with `Q ∈ [0,1]`; equivalently keep
`Σ Qi ωi Ai ≤ 1` so the normal's vertical component never goes negative and the surface never
passes through itself. Physically the wave breaks when crest particle speed `Aω` exceeds phase
speed `ω/k`, i.e. steepness `ε = Ak → 1`.

### 2.1b FFT ocean (Tessendorf)

**Phillips spectrum**:

```
P_h(k) = A · exp(−1/(kL)²) / k⁴ · |k̂·ŵ|² · exp(−k² ℓ²)
```

`k = |k|`, `ŵ` wind direction, `L = V²/g` (largest wave for wind speed `V`), `g = 9.8`,
`ℓ ≪ L` a small-wave cutoff. Tessendorf's worked example uses `V = 31 m/s`, `ℓ = 1 m`.
`|k̂·ŵ|²` removes waves perpendicular to wind (raise to `^6` for tighter alignment).

**Pierson–Moskowitz** (fully developed sea):

```
S(ω) = (α g² / ω⁵) exp[ −β (ω₀/ω)⁴ ],   α = 0.0081,  β = 0.74,  ω₀ = g/U₁₉.₅
peak at ω_p = 0.877 g / U₁₉.₅
```

**JONSWAP** (fetch-limited; Hasselmann et al. 1973):

```
S(ω) = (α g² / ω⁵) exp[ −(5/4)(ω_p/ω)⁴ ] γ^r
r = exp[ −(ω − ω_p)² / (2 σ² ω_p²) ]
γ = 3.3,  σ = 0.07 for ω ≤ ω_p else 0.09
α = 0.076 (U²/(F g))^0.22,   ω_p = 22 (g²/(U F))^(1/3),   F = fetch
```

JONSWAP with `γ = 1` reduces exactly to Pierson–Moskowitz.

**Initial spectrum**: `h̃₀(k) = (1/√2)(ξr + i ξi) sqrt(P_h(k))`, with `ξ` independent Gaussian
draws, mean 0, sd 1.

**Time evolution**: `h̃(k,t) = h̃₀(k) e^{iω(k)t} + h̃₀*(−k) e^{−iω(k)t}` — preserves the
conjugate symmetry `h̃*(k) = h̃(−k)` so the IFFT is real.

**Horizontal displacement (choppiness)**: `D(x,t) = Σ_k −i (k/k) h̃(k,t) e^{ik·x}`. Final
vertex offset `(x + λ D.x, h, z + λ D.z)`.

**Looping**: quantise `ω̄(k) = ⌊ω(k)/ω₀⌋ ω₀` with `ω₀ = 2π/T`, so the animation loops after
`T`.

**GPU IFFT**: use the **Stockham auto-sort FFT** to avoid the bit-reversal permutation. An
`N`-point transform is `log2(N)` butterfly passes; a **precomputed twiddle/butterfly texture**
encodes indices and twiddle factors per pass; ping-pong two buffers (Stockham is out-of-place,
which suits textures that cannot be simultaneously read and written). Rows, then columns.

**Cascades**: several FFTs at different patch sizes (e.g. 4 m, 128 m, 2048 m), summed, to get
detail without tiling.

### 2.1c Foam / whitecaps

The horizontal displacement folds the surface where it compresses. Compute the **Jacobian**:

```
Jxx = 1 + λ ∂Dx/∂x     Jzz = 1 + λ ∂Dz/∂z
Jxz =     λ ∂Dx/∂z     Jzx =     λ ∂Dz/∂x
det J = Jxx Jzz − Jxz Jzx
```

`det J = 1` where displacement is zero; it drops toward 0 at sharp peaks and goes **negative
where the surface passes through itself** — the unphysical fold is the whitecap. Foam is
generated where `det J` falls below a threshold. **Accumulate and decay**:
`F = max(F·decay, foamInjection(det J))`, then blend an artist foam texture masked by `F`.

### 2.1d UE5 Water plugin specifics

- **Water Bodies**: Ocean (infinite plane), Lake, River (spline flow), Custom. Spline-authored.
- **Waves**: the Water Waves asset sums **Gerstner banks**, not FFT.
- **Water Mesh + quadtree LOD**: "The level of detail of the water mesh tiles is handled by
  traversing a quadtree each frame to generate an optimized set of tiles… Each level of detail
  is made up of a concentric circle around the camera view… each lower level of detail is
  farther from the camera and contains half the number of vertices as the level that precedes
  it." `LODScale` sets morph distance, `TessellationFactor` density; morphing avoids popping.
- **Water Info Texture / Water Zone**: a top-down texture storing per-pixel surface height,
  flow and depth for a whole zone, sampled by materials, gameplay and buoyancy.
- **River flow maps**: a flow-map distorts UVs of the water normal; **two-phase flow blending**
  samples twice at offset phases and cross-fades to avoid the sliding artifact.
- **Depth/shore falloff**: distance-to-shore drives wave attenuation and shore foam.

### 2.2a Interactive ripples — 2D wave equation

```
u^{t+1}(i,j) = 2u^t(i,j) − u^{t−1}(i,j)
             + c² (Δt²/h²) ( u(i+1,j) + u(i−1,j) + u(i,j+1) + u(i,j−1) − 4u(i,j) )
```

Multiply by a **damping** factor slightly below 1. **CFL stability**: `c·Δt/h ≤ 1/√2` in 2D.
Implement with **ping-pong render targets** holding `(uᵗ, uᵗ⁻¹)`.

**Height→normal**: `N = normalize(vec3(u(i−1,j) − u(i+1,j), 2h, u(i,j−1) − u(i,j+1)))`.

**Injection**: objects write displacement into cells beneath them, amplitude from impact
velocity. **Moving domain**: centre the grid on the camera; when it scrolls, reproject by an
integer-cell shift and clear the newly exposed border so ripples stay world-anchored.

Sea of Thieves additionally **projects the depth buffer onto the surface mesh into the sim's
texture space**, so a character intersecting a waterfall occludes the stream and generates foam
at their feet.

### 2.2b Shallow water equations

```
∂h/∂t + ∇·(h u) = 0
∂u/∂t + (u·∇)u = −g ∇h
```

Solve with semi-Lagrangian advection of height and velocity plus a height-integration step;
add bottom terrain for shoaling. Depth-dependent wave speed and correct propagation, but the
**wrong deep-water dispersion** — a documented limitation for deep-water splashes.

### 2.2c FLIP/PIC

Incompressible Navier–Stokes `∇·u = 0`, `∂u/∂t + (u·∇)u = −∇p/ρ + F + ν∇²u`.

1. **P2G**: splat particle velocities to the MAC grid, `u_grid = Σ w_p u_p / Σ w_p`.
2. **Grid forces + pressure projection**: gravity, divergence, pressure Poisson, subtract
   `∇p/ρ`.
3. **G2P**: interpolate back. **PIC** overwrites (stable, dissipative); **FLIP** adds the
   *change* (energetic, noisy); blend `v_p = α v_FLIP + (1−α) v_PIC`. Niagara exposes the
   ratio: "0.0 = 100% PIC… stable, less accurate; 1.0 = 100% FLIP, accurate, less stable";
   ~0.75–0.95 is usable.
4. **Advect** particles through the grid velocity.
5. **Surface reconstruction**: build a level set from particles; **narrow-band FLIP** simulates
   only a shell near the surface.

### 2.2d Buoyancy

**Archimedes**: `F_b = ρ_water · g · V_displaced`, applied at the **centre of buoyancy** (the
centroid of the submerged volume).

Submerged volume by analytic shape, voxelization, or **pontoon sampling** — place pontoon
points on the body, sample the water height at each, and derive a fraction of buoyant force per
pontoon. Summing gives net force **and torque** (centre of buoyancy ≠ centre of mass), which is
what produces realistic rocking.

**Drag**: linear `−c₁v` plus quadratic `−c₂|v|v`; **added mass** accounts for accelerated
surrounding water. **Two-way coupling**: inject each pontoon's motion back into the ripple sim.

### 2.2e Splash spawning

Drive spawn rate from **impact velocity** and from the **Jacobian/foam** value: where `det J`
is strongly negative (breaking) or an object impacts above a speed threshold, emit at that
surface location. Impact speed sets initial velocity; a **crown** is a ring emitted with
outward+upward velocity. Render as **dithered masked flipbook sprites** — cheaper than
translucent, sorts against depth, and the flipbook starts when the particle spawns.

### 2.3 Water shading

**Absorption (Beer–Lambert)**: `T(d) = exp(−σ_a d)` per channel, red extinction larger than
blue → blue-green with depth. `d` = scene depth minus water surface depth.

**Screen-space refraction**: offset the scene-colour UV by the normal's xy scaled by distortion
and inverse depth. Mask objects in front of the water plane to avoid bleeding. Water IOR ≈ 1.33.

**Fresnel (Schlick)**: `F = F₀ + (1−F₀)(1 − n·v)⁵`, `F₀ ≈ 0.02`. **Specular/GGX** for sun
glint; handle high-frequency normal aliasing with **roughness-from-normal-variance**
(Toksvig/LEAN) so distant water does not sparkle-alias. Sea of Thieves uses a
**closest-point-on-sphere area specular** (Karis 2013) for a large low sun reflection.

**Reflections**: SSR (cheap, screen-limited), planar (accurate for a flat plane, 2× scene
cost), reflection captures (static), or Lumen.

**UE Single Layer Water**: exposes **Scattering Coefficients, Absorption Coefficients, PhaseG,
Color Scale Behind Water**; **Opacity** "controls the ratio between the volume's BSDF and the
surface's BRDF." Runs in a custom pass after base pass + deferred lighting, before regular
translucency; the fully lit scene and depth are its inputs. **Single depth layer**, so no
back-face/underwater correctness.

**Caustics**: projected textures, screen-space from the water normal, or the differential-area
method (brightness = originalArea/projectedArea of refracted light cells).

**Shoreline foam**: compare pixel depth to scene depth; small difference → depth-fade foam.

**Underwater**: exponential fog by depth, god rays, refractive distortion, bubbles, and a
surface-crossing transition post-process.

### Cross-references

- **Sea of Thieves** — Ang, Catling, Cifariello Ciardi & Kozin, *The Technical Art of Sea of
  Thieves*, SIGGRAPH '18 Talks (doi:10.1145/3214745.3214820). Verbatim on colour: "We blend
  between a deep water colour and a sub-surface water colour based on a combination of view
  angle, sun direction and a wave peak mask. The wave peak mask is generated from the FFT
  choppiness vertex offsets." On foam: "Foam is generated at wave peaks using the method
  described in the reference paper. It is also added around objects that intersect the water
  surface within a camera centered window using depth buffer comparisons. We progressively blur
  the result of the foam buffer with feedback."
- **Crest Ocean System** — Bowles, Zimmermann, Noris & Wang, SIGGRAPH 2017 Advances in
  Real-Time Rendering. LOD data "is stored in a multi-resolution format, namely cascaded
  textures that are centered at the viewer," each LOD the same resolution but a different world
  size, storing animated waves, dynamic waves, foam, flow, shadow and depth. **CDClipmap**
  meshing snaps verts to grid positions and computes a `lodAlpha` from taxicab distance to morph
  between LODs. Foam from the **determinant of the Jacobian**.
- **GodotOceanWaves, OceanFFT, fftWater, Arc Blanc (arXiv:2503.03326)** — open FFT
  implementations with Stockham FFT and Jacobian whitecaps.
- **Assassin's Creed III/Black Flag, Batman Arkham** — Gerstner water.

---

## Part 3 — Fire and smoke

### 3.1 Grid gas solver (Stable Fluids pipeline)

Fields: velocity `u`, temperature `T`, density/smoke `ρ`, fuel. Per step:

1. **Advect** (Stam 1999, semi-Lagrangian, unconditionally stable): `x_back = x − Δt u(x)`,
   sample with trilinear interpolation. Stable at large `Δt` but numerically smoothing. Reduce
   dissipation with **MacCormack/BFECC**: `φ̂ = A(φⁿ)`, `φ̃ = Aᴿ(φ̂)`,
   `φⁿ⁺¹ = φ̂ + (φⁿ − φ̃)/2`, then **clamp** to the range of contributing nodes (MacCormack is
   not unconditionally stable; the limiter is required).
2. **Buoyancy**: `f = (α(T − T_amb) − βρ) ŷ` — hot rises, heavy smoke sinks.
3. **Vorticity confinement** (Fedkiw, Stam & Jensen 2001): `ω = ∇×u`,
   `N = ∇|ω| / ‖∇|ω|‖`, `f_conf = ε h (N × ω)`. Optionally add curl noise.
4. **Divergence**: central differences.
5. **Pressure Poisson**: `∇²p = (ρ/Δt)∇·u`. **Jacobi** (GPU-friendly, 20–50 iters),
   **red-black Gauss-Seidel/SOR** (faster), or **multigrid** (best scaling). 3D Jacobi:
   `p = (p_i± + p_j± + p_k± − h² div)/6`.
6. **Project**: `u ← u − (Δt/ρ)∇p`.
7. **Boundaries**: solid walls set normal velocity 0 (free-slip); open boundaries let flow exit.
8. **Advect temperature/density/fuel**, apply cooling (often radiative `∝ (T/T_max)⁴`) and
   combustion.

### 3.2 Combustion (Nguyen, Fedkiw & Jensen 2002)

Track **fuel** and a **reaction coordinate**. Burn where fuel and temperature exceed ignition:
consume fuel, release heat, produce soot, and inject **gas expansion** as a divergence source at
the flame front — "a physically based model for the expansion that takes place when a vaporized
fuel reacts to form hot gaseous products," a divergence jump across the thin flame surface
captured with a level set. The **blue core** is the thin reaction zone.

### 3.3 Blackbody fire colour

**Planck's law**: `L(λ,T) = (2hc²/λ⁵) · 1/(e^{hc/(λ k_B T)} − 1)`. Integrate against CIE
colour-matching functions → XYZ → RGB. Intensity scales with **Stefan–Boltzmann** `∝ T⁴`. In
practice gate through a temperature→colour LUT for art control. Blue-white core → orange →
deep red edges.

### 3.4 Volumetric rendering

```
L = ∫₀ᴰ T(0,s) [ σ_a L_e(s) + σ_s L_in(s) ] ds,    T(0,s) = exp(−∫₀ˢ σ_t dt)
```

with `σ_t = σ_a + σ_s`. **Ray-march** the 3D texture; per step accumulate emission (blackbody
from `T`) and in-scattering, multiply transmittance by `exp(−σ_t Δs)`. For lighting, march a
**secondary ray toward each light**. A typical quality target is ~128 primary × ~6 light steps
per pixel, which is why empty-space skipping and temporal reprojection matter.

**Henyey–Greenstein**: `p(θ) = (1/4π)(1−g²)/(1 + g² − 2g cosθ)^{3/2}`. `g > 0` forward scatter
(smoke ≈ 0.2–0.8, clouds ≈ 0.8). The **Schlick approximation** is the cheaper real-time
substitute. Single scattering is usually enough; approximate multiple scattering with an
ambient term.

**Self-shadowing**: half-angle slice rendering, light propagation volumes, or deep shadow maps.
**Formats**: Sparse Volume Textures / Heterogeneous Volumes in UE5; OpenVDB/NanoVDB for baked
sims.

### 3.5 Flipbook path

- **Sub-UV flipbooks** indexed by age, with frame blending.
- **Motion-vector interpolation**: store per-texel flow between frames; warp A forward and B
  backward by the blend factor and cross-fade, eliminating stepping between sparse frames.
- **Soft particles**: fade alpha near opaque scene depth.
- **Six-way lighting** (Vlad Miller): bake how a plume is lit from **six directions**. Unity
  packs the six lightmaps into two RGBA textures with emissive in the second alpha. At runtime
  project each light direction onto the six basis directions and blend — the sprite "will react
  to any number and all types of dynamic lights… as well as indirect and ambient lighting" at a
  cost "comparable to a traditional lit sprite." Best for background/ornamental effects.

### 3.6 Fire + smoke in tandem

Share the same fields. **Fire = additive emissive** (temperature-driven, blackbody, high
intensity); **smoke = alpha/absorbing** (density-driven opacity). Accumulate smoke absorption,
then add fire emission. Temperature-driven opacity; dissolve/erosion via noise thresholding at
edges; **heat haze** = refraction offset proportional to temperature above the flame.

### 3.7 Noise

Perlin/Simplex, Worley, FBM. **Curl noise** (Bridson, Hourihan & Nordenstam, SIGGRAPH 2007) for
**divergence-free** turbulence: build a vector potential `ψ` and take its curl,

```
v = ∇×ψ = ( ∂ψ₃/∂y − ∂ψ₂/∂z,  ∂ψ₁/∂z − ∂ψ₃/∂x,  ∂ψ₂/∂x − ∂ψ₁/∂y )
```

by central differences. Because `∇·(∇×ψ) ≡ 0` the field has no sources or sinks, so particles
never collapse into points — unlike raw gradient-of-noise fields, which have drains everywhere.
**Boundaries**: modulate `ψ` with a ramp to zero based on distance to an obstacle, so the
boundary is an isocontour of `ψ`, its gradient is perpendicular, and flow is tangent (free-slip).
FBM of `ψ` gives multi-scale eddies. Dramatically cheaper than a Poisson solve while still
incompressible.

**Wavelet turbulence / noise upsampling** (Kim et al.; Schechter & Bridson) adds
high-frequency detail to a coarse sim.

---

## Part 4 — Implementation plan for Loom (Rust + ash/Vulkan)

### 4.1 Crate layout

- `loom-vfx-core` — GPU particle system: emitter/system descriptors, module graph → SPIR-V,
  particle buffers, free list, spawn info, indirect draw/dispatch.
- `loom-vfx-render` — sprite/mesh/ribbon/light/volume renderers, GPU sort.
- `loom-water` — Gerstner + FFT ocean, ripple render-target sim, foam, buoyancy, quadtree LOD,
  Single-Layer-Water-style shading.
- `loom-fluids` — 3D gas solver + 2D shallow water + optional FLIP; grid collections;
  volumetric ray-march.
- `loom-noise` — Perlin/Simplex/Worley/curl (CPU for baking; GLSL includes for GPU).

Maths via **glam**; **rustfft** for CPU-side spectrum baking; **noise-rs** for CPU noise; GPU
FFT and noise hand-rolled.

```rust
struct GpuParticleBuffer { positions: Buffer, velocities: Buffer, attrs: Buffer, // SoA
                           alive_list: Buffer, dead_list: Buffer, counters: Buffer }
struct EmitterDesc { sim_target: CpuOrGpu, capacity: u32, spawn_rate: f32,
                     stages: Vec<SimStage>, renderers: Vec<RendererDesc> }
struct SimStage { kind: ParticleOrDataInterface, iterations: u32, pipeline: ComputePipeline,
                  target: Option<GridOrRenderTarget> }
struct Grid3D { images: [Image; N_CHANNELS], resolution: UVec3, cell_size: f32 }
struct PingPong<T> { a: T, b: T, idx: usize }
```

### 4.2 Vulkan concerns

- **Compute pipelines**: one `VkPipeline` per stage shader; **descriptor indexing** for the
  variable set of grid channels.
- **std430**: a `vec3` still aligns to 16 bytes — prefer `vec4` or explicit padding. Keep
  attributes in separate tightly-packed arrays.
- **Push constants** for per-dispatch scalars (dt, grid res, iteration index, wind) — ≥128
  bytes guaranteed; larger data in a UBO.
- **Barriers**: `COMPUTE_SHADER_WRITE → COMPUTE_SHADER_READ` between dependent passes; `GENERAL`
  layout for read-write ping-pong images. Each Jacobi iteration needs a barrier, or ping-pong to
  avoid RAW hazards.
- **Indirect dispatch/draw**: sim writes live count → `vkCmdDispatchIndirect` for update,
  `vkCmdDrawIndexedIndirect` for rendering; expand the particle to a quad in the vertex shader
  (avoid geometry shaders).
- **3D images** (`VK_IMAGE_TYPE_3D`, storage + sampled) for grid fields; `R16F` to halve memory.
- **Async compute** for the fluid sim overlapping graphics, synced with **timeline semaphores**.
- **Double buffer** all simulation state.

### 4.3 Shader pseudocode

**Particle update**

```glsl
layout(local_size_x=64) in;
void main(){
  uint i = gl_GlobalInvocationID.x;
  if (i >= liveCount) return;
  uint id = aliveList[i];
  vec3 p = pos[id]; vec3 v = vel[id]; float age = attr[id].age;
  v += gravity * dt;
  v += curlNoise(p * freq, time) * noiseStrength * dt;
  float phi = texture(globalSDF, worldToSdf(p)).r;
  if (phi < radius){ vec3 n = sdfGradient(p); p += (radius-phi)*n;
                     v -= (1.0+restitution)*dot(v,n)*n; }
  p += v * dt; age += dt;
  if (age > lifetime){ uint slot=atomicAdd(deadCount,1); deadList[slot]=id; }
  else { pos[id]=p; vel[id]=v; attr[id].age=age;
         uint o=atomicAdd(drawCount,1); drawList[o]=id; }
}
```

**Ripple pass**

```glsl
void main(){ ivec2 c=ivec2(gl_GlobalInvocationID.xy);
  float u = texelFetch(curr,c,0).r; float uPrev = texelFetch(prev,c,0).r;
  float lap = uL+uR+uU+uD - 4.0*u;
  float uNext = 2.0*u - uPrev + c2*dt*dt/(h*h)*lap;
  uNext *= damping;
  uNext += injection(c);
  imageStore(next, c, vec4(uNext,0,0,0)); }
```

**FFT butterfly (Stockham)**

```glsl
void main(){ uint x=gl_GlobalInvocationID.x; uint row=gl_GlobalInvocationID.y;
  vec4 bf = texelFetch(butterflyTex, ivec2(x,stage),0);
  vec2 a = readComplex(bf.z,row); vec2 b = readComplex(bf.w,row);
  vec2 w = vec2(bf.x,bf.y);
  vec2 bw = vec2(w.x*b.x - w.y*b.y, w.x*b.y + w.y*b.x);
  writeComplex(x,row, a + bw); }
```

**Jacobian / foam**

```glsl
void main(){ ivec2 c=ivec2(gl_GlobalInvocationID.xy);
  vec2 dDx = (Dx(c+ix)-Dx(c-ix)); vec2 dDz = (Dz(c+iz)-Dz(c-iz));
  float Jxx = 1.0 + lambda*dDx.x; float Jzz = 1.0 + lambda*dDz.y;
  float Jxz = lambda*dDx.y;       float Jzx = lambda*dDz.x;
  float detJ = Jxx*Jzz - Jxz*Jzx;
  float foamNew = max(0.0, foamScale*(threshold - detJ));
  float foam = max(texelFetch(foamPrev,c,0).r*decay, foamNew);
  imageStore(foamOut,c, vec4(foam,0,0,0)); }
```

**Fluid steps (3D)**

```glsl
// advect
vec3 pos = cellCenter - dt*sampleVel(cellCenter);
imageStore(velNew, id, vec4(triSample(velOld,pos),0));
// divergence
float div = 0.5*((velR.x-velL.x)+(velU.y-velD.y)+(velF.z-velB.z))/h;
// jacobi pressure (iterate, ping-pong, barrier between)
float p = (pR+pL+pU+pD+pF+pB - h*h*div)/6.0; imageStore(pNew,id,vec4(p));
// project
vec3 grad = 0.5*vec3(pR-pL,pU-pD,pF-pB)/h;
imageStore(velNew,id, vec4(vel - grad,0));
```

**Volumetric ray-march**

```glsl
vec3 L = vec3(0); float T = 1.0;
for(int s=0;s<STEPS;s++){ vec3 p = ro + rd*(s*ds);
  float dens = texture(densityVol,toUV(p)).r;
  float temp = texture(tempVol,toUV(p)).r;
  vec3 emit  = blackbody(temp) * dens;
  float sigT = dens*extinction;
  float Tl=1.0;
  for(int j=0;j<LSTEPS;j++){ vec3 lp=p+lightDir*(j*lds);
     Tl *= exp(-texture(densityVol,toUV(lp)).r*extinction*lds); }
  vec3 inscat = lightColor*Tl*phaseHG(dot(rd,lightDir),g)*sigT;
  L += T*(emit + inscat)*ds; T *= exp(-sigT*ds);
  if(T<0.01) break; }
fragColor = vec4(L, 1.0-T);
```

### 4.4 Crates vs hand-rolled

**glam** for maths; **rustfft** for CPU-side spectrum/twiddle baking and offline validation;
**noise-rs** for CPU noise baking; **ash** + **gpu-allocator**. **Hand-roll on GPU**: the 2D/3D
IFFT (Stockham passes + twiddle texture), curl noise, all sim stages, ray-march. There is no
drop-in pure-Rust Vulkan FFT worth putting in the hot path — write the butterfly compute shader
and validate it against rustfft on the CPU.

### 4.5 Milestone roadmap

1. **GPU particle core** — SoA buffers, free list, spawn/update compute, indirect draw. *Done*:
   1M GPU particles with gravity + curl noise at stable frame time. *M*
2. **Sprite + mesh renderers + GPU depth sort.** *Done*: sorted translucent sprites, instanced
   meshes. *M*
3. **Gerstner water surface** — sum-of-Gerstner in the vertex shader with analytic normals.
   *Done*: believable animated ocean, tweakable wave bank. *S–M*
4. **Interactive ripple render-target + buoyancy** — 2D wave-eq ping-pong, object injection,
   height readback, camera-following domain with reprojection. *Done*: a sphere dropped in water
   makes expanding ripples and bobs with correct rocking. *M*
5. **FFT ocean + foam** — Stockham IFFT (height + choppiness + slope), cascades, Jacobian foam.
   *Done*: non-tiling ocean with whitecaps at wave pinches. *L*
6. **Splash/spray particles** — spawn from Jacobian + impact events. *Done*: breaking crests and
   impacts throw sorted splash sprites. *M*
7. **3D gas solver** — Stable Fluids + MacCormack + Jacobi + vorticity confinement + buoyancy +
   combustion. *Done*: rising plume and a flame with turbulent detail on 128³. *L*
8. **Volumetric rendering** — ray-march density/temperature, blackbody fire + Beer–Lambert smoke
   + HG scattering + self-shadow. *Done*: lit, self-shadowed smoke and emissive fire composited
   with scene depth. *L*
9. **Six-way-lighting flipbook path** (parallel cheap track). *Done*: baked plumes react to
   dynamic lights. *M*

### 4.6 Budgets

- **Grid memory**: one `float32` channel at 128³ ≈ **8 MB**; 256³ ≈ **64 MB**. A gas sim needs
  ~6–8 channels → 128³ ≈ 50–64 MB, 256³ ≈ 0.4–0.5 GB. **Use R16F** to halve. Real-time gas sims
  are commonly 64³–192³.
- **Particles**: thousands to low-millions on GPU; splash systems hundreds–thousands live.
- **FFT ocean**: 256×256 per cascade, 2–4 cascades is a Sea-of-Thieves-class target.
- **Tricks**: sparse/narrow-band grids, dual-resolution (coarse velocity, fine density),
  half-precision, temporal amortization, tile/quadtree LOD, off-screen culling.

### 4.7 Pitfalls

- **std430 `vec3` padding** — the #1 buffer-corruption bug; pad to `vec4`.
- **Reading a ping-pong buffer you are writing** — always double-buffer; barrier between Jacobi
  iterations.
- **Missing pipeline barriers** → races that look like flicker.
- **CFL violation** in the explicit ripple sim (`c·Δt/h > 1/√2`) → it explodes; clamp `c` or
  substep.
- **Gerstner loop artifacts** — enforce `Σ Qi ωi Ai ≤ 1`.
- **FFT conjugate-symmetry errors** — get `h̃*(−k)` wrong and the IFFT yields garbage; the
  `[−N/2, N/2)` index range versus the standard non-negative FFT range needs care at Nyquist.
- **Depth-buffer collision is view-dependent** — use SDF collision where correctness matters.
- **JONSWAP `γ^r` double-exponential errors**, and conflating Phillips `L = V²/g` with the
  surface-tension `L`.
- **Buoyancy oscillation** — too-stiff buoyancy plus too-little drag jitters; add quadratic drag
  and enough pontoons.
- **Volumetric banding** — jitter the ray start per pixel. **Water normal aliasing** — use
  roughness-from-normal-variance.

---

## Recommendations (staged)

1. **Build the GPU particle core first (1–2).** Everything else depends on it. Benchmark to
   advance: 1M particles updating < 1 ms.
2. **Ship Gerstner before FFT (3), then the interactive ripple RT + buoyancy (4).** This
   delivers the "sphere splashes and it reacts" demo with the least code. Advance to FFT once
   Gerstner tiling is visually objectionable at your camera distances.
3. **Adopt Crest's cascaded-LOD data-texture architecture** as the reference for the FFT stage —
   the best-documented, most directly portable published design. Cross-reference the Sea of
   Thieves talk for colour and foam art direction.
4. **For fire/smoke, ship the flipbook + six-way-lighting path first (9)**; treat the 3D solver
   plus volumetrics (7–8) as a hero-effect feature. Only invest in the solver if you need
   interactive, world-reacting fire that flipbooks cannot fake.
5. **Validate each solver on CPU with rustfft/nalgebra before porting to compute** — a bit-exact
   CPU reference makes GPU debugging tractable, especially for the FFT and the pressure solve.
6. **Read the primary sources before implementing each stage.**

**Primary sources**: Tessendorf, *Simulating Ocean Water* (Phillips/h₀/dispersion/choppiness);
GPU Gems 1 Ch.1 (Finch, Gerstner + normals); GPU Gems Ch.38 (Harris) and GPU Gems 3 Ch.30
(2D/3D GPU fluids, MacCormack, ray-march); Stam 1999 *Stable Fluids*; Fedkiw, Stam & Jensen 2001
*Visual Simulation of Smoke*; Nguyen, Fedkiw & Jensen 2002 *Physically Based Modeling and
Animation of Fire*; Bridson, Hourihan & Nordenstam 2007 *Curl-Noise for Procedural Fluid Flow*
and Bridson's *Fluid Simulation for Computer Graphics*; Dupuy & Bruneton *Real-time Animation
and Rendering of Ocean Whitecaps* and Tessendorf *Whitecap Phenomenology*; Ang, Catling,
Cifariello Ciardi & Kozin, *The Technical Art of Sea of Thieves*, SIGGRAPH '18 Talks; Crest Ocean
System docs/repo (SIGGRAPH 2017); Arc Blanc (arXiv:2503.03326); Epic's Niagara, Niagara Fluids,
Water Meshing and Single Layer Water documentation; Mei, Decaudin & Hu 2007 *Fast hydraulic
erosion simulation*.

---

## Caveats (from the author)

- **Epic's exact internal implementations are largely undocumented.** Statements about Niagara
  and Water internals — free-list layout, exact quadtree traversal, Water Info Texture channel
  packing, the precise Single Layer Water maths — are the best-known public equivalents. Treat
  them as faithful reconstructions, not Epic source. The UE Water surface uses **Gerstner banks
  by default, not FFT**; the FFT ocean here is the Sea of Thieves/Crest route.
- Some figures (a fan Sea-of-Thieves-style recreation at "~90 FPS on a GTX 970", memory numbers)
  are secondary/community sources — order-of-magnitude guides, not Epic-verified benchmarks.
- **Six-way-lighting channel packing differs between Unity and UE**; verify against the chosen
  baker.
- Arc Blanc's printed JONSWAP `ω_p` appears to drop the `^(1/3)` exponent (likely a typesetting
  artifact); use the canonical Hasselmann 1973 form `ω_p = 22 (g²/(U F))^{1/3}`.

---

## Where this meets Loom as it stands

Recorded so the next reader does not have to rediscover it. Verified against this tree:

- **P3 water is the next engine phase** and is unstarted; `loom_water` already exists with a
  `WaterBody` component and a wave list, so milestone 3 is an extension rather than a new crate.
- **Particles are CPU-side today** (`loom_particles`), stepped at a fixed tick. Milestone 1 is
  therefore a real new subsystem, and it is the prerequisite the report names.
- **Rain drops are already GPU-stateful** (ADR 0017) with a compute pass, a collision volume and
  an indirect draw whose count never leaves the device. That is a working, shipped example of
  exactly the pattern milestone 1 generalises — read `loom_rain` before designing it.
- **The engine already has curl-free noise discipline**: `loom_field::noise` is a frozen ABI,
  written in Rust and Slang side by side and compared exactly, because a crate bump must never
  change the sim hash (ADR 0006). Any noise this work needs belongs there, not in a new crate.
- **`loom_field` cannot express this.** ADR 0006's `Expr` tree is a scalar-field language with
  no neighbourhood search and no struct output, which is why the deferred grass compute pass
  needed its own ADR. A grid solver needs the same decision taken first.
