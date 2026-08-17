# Niagara, Loom's fire, and what is actually worth building

*Written 16 Aug 2026. Every claim about Loom's code below was read from this working tree at `62f9ebe` this session; file:line references are given so you can check them. Every Niagara claim is either quoted from Epic's docs (linked) or flagged as unconfirmed. Numbers from the research passes are marked as such.*

**If you read one section, read §3.** It is four changes to one function, it moves two golden references, it needs no ADR, and I believe it carries most of the win. §4 is real but it is not what is wrong with your fire.

---

## 1. How Niagara works

### 1.1 The object model

Four assets, plus two things that are deliberately *not* assets:

| Thing | Asset? | Is | Reused by |
|---|---|---|---|
| **System** | yes | a list of Emitters + a system-level stack + a Sequencer timeline | placed in a level |
| **Emitter** | yes | its own stack + its renderers + emitter properties | *inherited* by Systems |
| **Module** | yes (a `NiagaraScript`) | a graph or an HLSL body | any stack group of any emitter |
| **Dynamic Input** | yes (also a script) | a sub-expression feeding one module input | any input of matching type |
| **Renderer** | no — an *item* | how simulated data becomes pixels | per-emitter |
| **Data Interface** | no — a *parameter type* | access to external state | any module |

Epic draws the line explicitly: *"A module is an item, but an item is not a module. Modules are editable assets a user can create. Items refer to parts of a system or emitter that the user cannot create. Examples of items are system properties, emitter properties, and renderers."* ([Key Concepts](https://dev.epicgames.com/documentation/en-us/unreal-engine/key-concepts-in-niagara-effects-for-unreal-engine), verified this session)

An Emitter placed into a System is **inherited, not copied**: the System-local instance overrides module inputs and may add modules, and edits to the source emitter propagate downstream. This is why Niagara has an emitter/module *versioning* system — propagation can break the systems that inherit.

### 1.2 The parameter map — the single idea everything else rests on

Everything flowing through a stack is one value: a **Parameter Map**. Epic: *"Our parameter map is the particle payload that carries all of the particle's attributes."* (verified verbatim this session)

A module's graph is literally `Input Map → [Map Get … math … Map Set] → Output`, with one white parameter-map wire running left to right. `Map Get` reads a named attribute; `Map Set` writes one.

The consequence: **there is no fixed particle struct.** `Particles.Position` exists because some module wrote it. A particle has `Particles.MeshOrientation` only if a mesh-orientation module is in the stack. The emitter's payload layout is the union, over the compiled stack, of every attribute any module writes. That is the mechanism, and it is why any type — structs, matrices, bools — can be a particle attribute.

*(I could not confirm whether the compiler strips attributes that are written but never read. Treat payload minimisation as unverified.)*

### 1.3 Namespaces and the scoping rule

Namespaces are containers for hierarchical data: `Engine.DeltaTime`, `Emitter.Age`, `Particles.Position`. The access matrix is compiler-enforced and I verified it verbatim against Epic's Key Concepts page:

| Module group | Reads from | Writes to |
|---|---|---|
| **System** | System, Engine, User | System |
| **Emitter** | System, Emitter, Engine, User | Emitter |
| **Particle** | System, Emitter, Particle, Engine, User | Particle |

Read the diagonal: **you read everything above you and write only your own level.** That is the entire scoping model. It is one table, it is an afternoon to implement, and it is what makes a module reusable — a module touching only `Particles.*` and reading `Engine.DeltaTime` cannot be broken by being dropped into a different emitter.

- **`User.`** is the public API of a System asset: settable only from outside (Blueprint/C++), writable by nothing inside.
- **`Engine.`** is engine-provided and read-only everywhere: delta time, owner transform, position.

Inside a module graph there are further namespaces — conventionally a `Module.` namespace for a module's own declared inputs, which is the mechanism by which a declared input becomes a per-instance stack widget (so two modules can each have an input called `Scale`). **I could not verify the exact spellings** (`Module.`, `Local.`, `Output.`, `Transient.`, `StackContext`) against a primary source. The concept is certain; the names are not.

### 1.4 The stack

An emitter executes, top to bottom:

1. **Emitter Spawn** — once, on the frame the emitter comes into existence.
2. **Emitter Update** — every frame. **Spawning is decided here**, by modules: `Spawn Rate`, `Spawn Burst Instantaneous`, `Spawn Per Frame`, `Spawn Per Unit`, `Emitter State` (looping/lifecycle), `Scalability` (distance/visibility culling).
3. **Particle Spawn** — once per particle, on its birth frame.
4. **Particle Update** — every frame, per particle.
5. **Event Handler** — §1.7.
6. **Render** — §1.8.

Above these sits **System Spawn / System Update**, shared by every emitter in the system. Epic's summary: simulation *"flows from the top of the stack to the bottom, executing programmable code blocks called modules in order, with every module assigned to a group that describes when the module is executed."*

The Particle Update catalogue gives the texture of what a module is. Forces: `Acceleration Force`, `Curl Noise Force`, `Drag`, `Gravity Force`, `Limit Force`, `Line Attraction Force`, `Point Attraction Force`, `Vortex Force`, `Wind Force`. Location: `Box Location`, `Cone Location`, `Sphere Location`, `Torus Location`, `Jitter Position`, `Static Mesh Location`. Colour: `Color`, `Scale Color`, `Scale Color by Speed`. Kill: `Kill Particles`, `Kill Particles in Volume`. Plus `Collision`, `SubUVAnimation`, `Sample Texture`, `Do Once`.

**Why this composes and a fixed field list does not.** Loom's `ParticleEmitter` has `gravity`, `drag`, `turbulence`, `wind_response` (verified, `crates/loom_scene/src/components.rs:349-477`). Niagara has `Gravity Force`, `Drag`, `Curl Noise Force`, `Wind Force`. Superficially the same. Four structural differences:

- **Order is authored.** `Drag` before `Gravity Force` ≠ after. In Loom the order is whatever `loom_particles::System::step_in_wind` does, forever, and it is invisible to the author.
- **Multiplicity.** Two `Point Attraction Force` modules with different centres. A fixed component has one `gravity` scalar and can never have two attractors.
- **The payload is open.** A new module invents `Particles.Charge`; a second reads it; no engine change. Under a fixed struct that is a layout change, a serialisation change and a shader change.
- **One uniform composition point.** Modules speak only parameter-map-in / parameter-map-out, so any module goes anywhere in a stage. No interface, no registration, no dispatch table.

### 1.5 Compilation

A module is authored as a node graph or as HLSL (the **Scratch Pad** produces a module local to the system; "Export to Library" promotes it to a shared asset). **The compiled artefact is one script per stage** — every module in, say, Particle Update is inlined in stack order into a single script. Two backends: on GPU the compiler translates the graph into HLSL compute; on CPU it compiles to bytecode for a SIMD interpreter Epic calls the **VectorVM** (name confirmed by Epic's public roadmap item "Niagara - CPU VectorVM (Experimental)"; the bytecode format is not publicly documented that I found).

**One source, two backends, and the artist rewrites nothing to switch.** This is the same architecture as ADR 0006 one level up: not a scalar expression, but a statement sequence over a named record. Three implications for a Loom port: the stage compiles to flat straight-line code (textual concatenation, not dynamic dispatch — well within `build.rs`); the payload layout must be resolved *before* codegen because it fixes struct offsets on both sides; and compilation must be cached by graph hash, because `loom run --watch` would otherwise regenerate Slang on every keystroke.

### 1.6 CPU vs GPU, and Data Interfaces

An emitter has a **Sim Target**: `CPUSim` or `GPUComputeSim`. Same modules either way. What GPU gives up: **events do not work** (*"Events only work with CPU simulation"*); **Fixed Bounds become mandatory** because the CPU cannot read how big the effect is; and there is no cheap readback, so nothing GPU-side can feed gameplay. What it buys: counts the CPU cannot reach, and Simulation Stages. Community rule of thumb: over ~1000 particles GPU, 1–100 CPU (small counts pay GPU's fixed overhead for nothing).

A **Data Interface** is a *parameter type* whose value is a set of functions providing external data access. You declare one as a module input and call functions on it. The implementation shape — from community documentation of the C++ API, which I did **not** verify against Epic's source — is a class deriving `UNiagaraDataInterface` implementing `GetFunctions` (signatures), `GetVMExternalFunction` (CPU), and `GetParameterDefinitionHLSL` / `GetFunctionHLSL` (GPU). **That last pair is the interesting part: a data interface injects its own generated HLSL into the stage shader. It is a codegen plugin, not a runtime call.**

Confirmed interfaces include Grid2D, Grid3D, Neighbor Grid 3D, Distance Field, Static Mesh Collisions, Particle Attribute Reader, Curl Noise, Render Target 2D, Collision Query, Audio Spectrum.

**The design lesson is the boundary, not the machinery: "add a behaviour" and "read external world state" are different extension axes.** Collapsing them forces every world query into the module vocabulary.

### 1.7 Events

Generators are ordinary Particle Update modules (`Generate Collision Event`, `Generate Death Event`, `Generate Location Event`) writing a payload into an event set. Handlers are an extra stack group on the *receiving* emitter with a matching receive module; a handler either spawns N particles per event and runs its stack on them, or runs on existing particles. Requires **Persistent IDs** and is **CPU-only**.

### 1.8 Simulation Stages — the biggest idea, and the least obvious

A Simulation Stage is a GPU-only feature enabling multiple ordered Spawn/Update passes per frame. Two properties define one:

- **Iteration Source** — `Particles` or `Data Interface`. Particles means one thread per particle. **Data Interface means one thread per *cell*** of a Grid2D/Grid3D/Neighbor Grid/Render Target — the dispatch is sized by the grid, not the particle count.
- **Num Iterations** — the number of times the stage runs in a row before the next stage (`UNiagaraSimulationStageGeneric::Iterations`, confirmed from an official API page).

Grid access is `GetFloatValue(Attribute, IndexX±N, IndexY±N)` / `SetFloatValue(...)` with double buffering — read A, write B, swap.

Three distinct things this enables that one update loop cannot: iteration over something that is not particles; **multiple ordered passes in one frame with a barrier between**, which any stencil operation (a Laplacian, a pressure projection) structurally requires; and iterative convergence (a Jacobi solve is 20–40 runs of one kernel).

**Reframed for Loom: a simulation stage is "an arbitrary compute pass that the render graph schedules, authored as content rather than as engine code."** Loom's graph already owns buffer barriers and dispatches compute (`rain_sim.slang`, ADR 0017). The gap is not capability. The gap is that only an engine commit can add a pass.

### 1.9 Renderers

An emitter may have several renderers, so one particle set draws as sprites *and* lights *and* ribbons. Verified this session from Epic's Render Module Reference: **Sprite**, **Mesh**, **Ribbon**, **Light**, **Component**, **Decal**. There is **no Volume Renderer** on that page — volumetric Niagara Fluids output goes through Sparse Volume Textures / Heterogeneous Volumes, which are separate rendering features. Do not build against a "Niagara Volume Renderer" on anyone's word.

Two details that matter later:

- **Sprite Renderer** — `Sub Image Size` is the columns × rows of the atlas; `Sub UV Blending Enabled` blends the sub-image UV lookup with its next adjacent member (both quoted verbatim, verified). The frame *index* is chosen by the `SubUVAnimation` **module**; the renderer only samples it. Animation is content; sampling is an item.
- **Light Renderer** — *"Radius Scale — this factor is used to scale each particle light radius"*, *"Color Add — a static color shift applied to each rendered light"*, plus `Affects Translucence` and `Use Inverse Squared` (all verified verbatim). Position, colour and radius are **bindings onto particle attributes** — no separate light entity, no sync problem, and the knobs exist precisely because the light should not literally equal the sprite.

### 1.10 Niagara Fluids

An optional plugin: 2D Gas, 2D Liquid, 3D Gas, 3D Liquid, Shallow Water. 2D is *"optimized for games"*; 3D is *"designed for cinematics."* 3D water uses PIC/FLIP.

**The architectural punchline:** Epic says users can modify these *"without needing to write code, plugins, or data interfaces."* Fluids is **content**, built on simulation stages and grid data interfaces. Epic did not add a fluid solver to the engine; they added stages and then authored a solver on top. That is the strongest available evidence that stages are the right abstraction.

**And the cost:** Epic's own words — fluid sims *"can be heavy"* and *"may result in a GPU crash on Windows."* The documented escape hatch is to bake to a flipbook (the Flipbook Baker defaults to 8×8 / 64 frames at 1024²). Take that escape hatch seriously as a feature rather than an admission of defeat; it is how expensive simulation reaches shipping content.

---

## 2. What is actually wrong with Loom's fire

I read `flameColor`, the `FIRE_*` constants, `particleFragmentMain`, the particle pipeline state, `campfire.loom` and `lanternhead.loom`. The diagnosis is mechanical, and none of it is a bug — every piece is the specified output of a constant that was tuned correctly to solve a problem that no longer exists.

### 2.1 The flame is 100% transparent, so the background shows through its body

`assets/shaders/scene.slang:2527`:

```
return float4(fireRamp(tau) * cover * in.color.a, 0.0);
```

and `:2537`:

```
return float4(fire.rgb * (1.0 - fogF), 0.0);
```

The alpha is the literal constant `0.0`, in both places. The particle pipeline is premultiplied — `src_color = ONE`, `dst_color = ONE_MINUS_SRC_ALPHA` (`crates/loom_render/src/renderer.rs:3636-3640`, with a comment saying exactly why: *"a particle that then reports alpha 0 contributes its colour and occludes nothing, which is exactly additive blending"*). Depth write is off (`:3623`).

So `dst` is preserved at 100% **inside the flame core**. On `campfire`'s black sky nothing shows through because there is nothing there. On `lanternhead`'s lit deck, the stone's grain, its luminance variation and any geometric edge behind the flame are all visible *through* the fire. That reads as an orange decal painted onto the wall, not as an object in front of it. **This is the single largest contributor and it is one constant.**

### 2.2 The gaps between tongues show 100% background — by construction

Three constants, all read this session:

- `FIRE_T1 = 1.05` (`:2392`). Its own doc comment states the arithmetic: `fireFbm` normalises by amplitude sum and `loom_value_noise` is on [0,1), so the field's supremum is exactly 1.0. `FIRE_T1` is deliberately **above** it so extinction is *certain* rather than probabilistic.
- `FIRE_GAP = 0.72` (`:2406`). Same arithmetic, same reasoning, quoted: at the old 0.30 *"the gaps between tongues were a fifth lit, every tongue joined its neighbours, and the fire was one connected blob."*
- `FIRE_CONE = 0.55` adds `cone * 0.22` to the threshold radially.

The threshold is an **offset**, not a multiply — deliberately, per the comment at `:2496`, because a multiply's soft edge *"is precisely what turns a fire back into a fireball."*

Net: the tongues are genuinely, topologically **disconnected components** of a level set, and the space between them is pure background at full strength.

### 2.3 Every surviving component has a hard, full-brightness boundary

`FIRE_TAU_FLOOR = 0.30` (`:2411`), with its comment: without the floor *"the outermost pixels read as black and full colour arrives several pixels in — the shader paints the very halo it exists to remove."*

So the dimmest colour any flame pixel emits is `fireRamp(0.30 × (1 − 0.42h))`. Evaluating `fireRamp` (read at `:2455-2463`) gives linear **(0.35, 0.051, 0.0035)** at the base and **(0.24, 0.018, 0.0014)** at the tip. That value arrives within one pixel of the boundary — `cover = saturate(e / fwidth(e))` is a one-pixel analytic antialias and nothing more.

### 2.4 The three together

**The frame is N disjoint, hard-edged, near-uniformly-bright orange regions, separated by gaps that show the background unattenuated, over a core that is itself fully transparent.**

It reads as one fire *if and only if* the background is dark enough that the eye supplies the connection and the see-through is invisible. `campfire.loom` authors that: `Light.intensity = 3.0` warm and low, a near-black sky. `lanternhead.loom:488-493` records the failure in its own words — *"On the open deck it was three detached orange shards… A fire in this engine needs a dark backdrop, and that is a composition constraint rather than a bug."*

That sentence is a correct description of the current shader and an incorrect description of the technique's ceiling.

### 2.5 The constraint that produced all of it is gone — but be precise about what was revisited

The comment block at `scene.slang:2333-2349` justifies the single quad: the target *was* `R8G8B8A8_SRGB`, and measured overdraw pinned red after **2.6 overlapping particles** against ~30 alive. ADR 0018 deleted that target; the frame is `R16G16B16A16_SFLOAT` with one tonemap.

**Correction to the framing I was given.** The claim "nobody revisited it" is only half true. `fireRamp`'s top rung now reads `float3(2.60, 1.900, 1.200)` (`:2462`) and its doc comment explicitly says ADR 0018 is what removed the cap. `campfire.loom:152` reads `intensity = 3.0`. **The ramp and the light were revisited. The level-set topology and the zero alpha were not** — and those are what produce the shards. The comment block at `:2337` still describes the 8-bit target in the present tense and is now stale; so is the identical passage in `campfire.loom:162-167` and in `ParticleEmitter::flame`'s doc comment (`components.rs:433-445`).

### 2.6 What is fixable inside the level set, and what is not

The **silhouette machinery is good and should be kept**: domain warp amplitude set by the fold condition, threshold above the supremum so extinction is certain and the crossing steep enough to pinch rather than fade, the fuel lobes, the vertical squash applied *after* the warp. That is real, well-argued craft and nothing below throws it away.

The **shading is the ceiling**, and the reason is one sentence: **a level set on a plane renders a slice, but fire is a line integral.** A view ray through real fire crosses many small hot pockets and sums them; that sum is smooth and connected even when every individual iso-surface is fragmented. Loom takes one slice of one field at one depth and thresholds it. Every knob that reconnects the shards *inside that formulation* — lower `FIRE_HZ`, lower `FIRE_GAP`, wider `cover` — works by removing the detail that makes it read as fire, and converges back on the fireball the design was written to escape.

**You do not have to leave the quad to break that ceiling.** §3 items 1–4 give the flame optical depth and something of its own in the gaps; item 5 integrates the same field along depth inside the same quad. That is the structural fix and it is still one function.

### 2.7 The scale of the blast radius, which is the reason to do this first

- **Only two scenes in the repo author `flame = true`**: `assets/test/campfire.loom` and `assets/test/lanternhead.loom` (verified by grep across `assets/`). A fire shader change moves exactly two golden references out of 26.
- **`campfire`'s fire is one particle** — `burst = 1`, `rate = 0.0`, `lifetime = 110.0`, `speed = 0` (`campfire.loom:173-181`). A particle-*system* redesign for a one-particle effect is solving a different problem than the one on the table.
- By contrast, **8 of the 26 `GOLDEN` entries carry a `ParticleEmitter`** (lanternhead, smoke, explosion, windy, proving_ground, campfire, splash, homestead — cross-checked between `xtask/src/main.rs:169-357` and a grep of `assets/`). Anything that changes the simulation's force order moves all eight at once.

---

## 3. Fix the fire without rewriting anything

> ## ⚠ STATUS: ITEMS 1–5 ARE BUILT. READ THIS BEFORE ACTING ON THE LIST BELOW.
>
> This section was written against the pre-ADR-0020 shader and is kept for its
> reasoning, not as a work list. **`ADR 0020` took item 5** — the march — and
> item 5's own last line is why 1–3 came with it: the integral *subsumes* them.
> Against `assets/shaders/scene.slang` today:
>
> - **Item 1 is done at both sites.** `flameColor` returns
>   `(1 - T) * FIRE_OPACITY * in.color.a`, and the fog return fogs the alpha as
>   well as the colour rather than passing it through — a flame that kept its
>   opacity while its emission was attenuated would fade to a black silhouette
>   at range. **The discard trap this section flags was real and is closed**:
>   the predicate is now `rgb sum <= 0.0008 && a <= 0.0008`.
> - **Item 2 is done by deletion.** The hand-built `FIRE_GLOW*` veil was written
>   and then removed, because a ray through a gap between tongues still crosses
>   material at other depths — the same term, derived instead of authored, and
>   it parallaxes with the camera instead of being pinned to the billboard.
> - **Item 3 is done, and half of it turned out to be wrong.** Base cooling is
>   `FIRE_TAU_BASE`/`FIRE_TAU_BASE_H`; `FIRE_TAU_FLOOR` is gone, so the ramp's
>   bottom rungs are reached. The **flank** cooling this item asked for was
>   built and then deleted: a grazing ray accumulates little `heat` because it
>   genuinely crosses little material, so the term was double-counting.
> - **Item 4 is done and it needed engine work, not the "zero shader work,
>   scene-side" this section predicted.** `Light.intensity` is a static scalar
>   with no expression language behind it, so the modulation is a new
>   `Light.flicker` field evaluated in `gather_lights` from the `seconds` the
>   environment already carries. The amendment held exactly: the clock is the
>   tick timebase, and the hue shifts with the brightness.
>
> **Items 6, 7 and 8 are still open** and are still authoring/measurement work.

**This section can carry most of the win, and I say that plainly.** Items 1–4 are one function, roughly fifteen lines, two golden references, no ADR, no locked decision touched. Do them in one commit and look at `lanternhead`'s deck before considering anything in §4.

Ranked cheapest-first. Each is a change to `flameColor` in `assets/shaders/scene.slang` unless stated.

### 1. Return a non-zero alpha — 3 lines, and the biggest single win

The pipeline is already premultiplied `ONE / ONE_MINUS_SRC_ALPHA`, so returning `alpha = cover * f(e)` makes the flame occlude what is behind it with **no pipeline change, no new pass, no sort concern** (campfire's flame is one particle; the particle stream is already CPU-sorted back-to-front at `renderer.rs:1625`).

Two call sites, not one: the `float4(..., 0.0)` at `:2527` **and** the fog return at `:2537`, which currently hard-codes `0.0` again.

*Looks like:* the flame's core stops being a window onto the stone behind it. Fire becomes an object.

*Watch:* `particleFragmentMain` discards on `fire.r+fire.g+fire.b <= 0.0008` (`:2535`). Once alpha carries meaning, that RGB-only test is the wrong extinction predicate — test the alpha too, or the discard boundary itself becomes a visible edge.

### 2. Give the gaps something of the fire's own — a dim unthresholded continuum term

The shards *are* the gaps showing background at 100%. Add a separate, low-frequency, low-amplitude emission derived from the already-computed `d` and the envelope (`radial`, `cone`, `h`) — **not thresholded, no level set** — occupying the whole flame envelope. Contribute it to both rgb and alpha.

This is the "back layer for ambient glow" that every layered-fire reference specifies, done inside the quad instead of as a second draw.

*Looks like:* three shards on the deck become one glowing body with bright tongues inside it. **Highest value per line on this list.**

*Cost:* a few lines and one `lerp`. `d` is already in hand; no extra fbm.

### 3. Cool the base and the flanks

`FIRE_TAU_H = 0.42` cools the tip and nothing cools the base or the flanks, and `FIRE_TAU_FLOOR = 0.30` means the ramp's bottom two rungs — `(0.00,0,0)→(0.22,0.012,0.001)` and `→(0.48,0.09,0.006)` — are effectively dead code. Multiply `tau` down near `h ≈ 0` and near the level-set boundary so those rungs are actually reached.

This is what "grade the core hotter than the edges" means in practice, and it is also physically right: at the fuel line a flame is dimmer, less saturated and narrower.

*Looks like:* interior structure. Today the fire is one orange value with holes in it.

*Cost:* two terms. **Do this together with 1 and 2 — they are three edits to one shading path and separating them buys nothing.**

### 4. Flicker the `Light`, and shift its hue with intensity — scene-side, zero shader work

`campfire.loom:152` is `intensity = 3.0`, constant. Firelight flicker is one of the strongest night cues there is, and Epic's documented Niagara approach is literally short-lived light particles producing it.

**Amendment that is not optional:** derive the modulation from the tick timebase (`weather.z`, the same clock `FIRE_RISE` uses at `:2482`), never the wall clock — never-do #8. And shift hue with it (dimmer → redder), not just scale, because that is what a flame does.

*Cost:* one scalar. *Watch:* it moves `campfire`'s reference, which is already tick-dependent via `--sim 200`. One bless, readable in `MANIFEST.txt`.

**Stop here and look at the result.** Run `cargo xtask flythrough` on `lanternhead` — a still cannot tell you whether the tongues cohere *in motion*, which is the actual question.

### 5. Raymarch the field through the quad's depth — only if 1–4 are not enough

6–12 taps of the existing `fireFbm` along the view ray, accumulating emission and transmittance (`exp(-σ·Δt)`), emitting premultiplied colour and `1 − T`. This is the structural fix from §2.6: it turns a slice into an integral, which is what glues fragmented iso-surfaces into a connected object, and it gives real depth parallax as the camera moves — the current flame has none. It subsumes items 1, 2 and 4 of the shading list.

Reuse `FIRE_RANGE`'s octave-retirement logic per step or it will twinkle.

*Cost:* 6–12× the fbm ALU **in flame pixels only**. For context, CLAUDE.md records grass at 0.054 ms for 45,460 blades and the whole forward pass of every scene at 0.05–0.11 ms; a flame covering a few percent of the frame at 10× ALU is comfortably affordable. A note in the shader, not an ADR — it changes no locked decision.

**Ship it in a separate commit from 1–3.** That is the P2 slice-7 lesson verbatim: two tools measured as one is not a measurement.

### 6. Couple the smoke to the flame — authoring, not engine

Smoke is born where the flame *dies*, not at the fuel — in Loom, at the height where the `FIRE_T1` threshold crosses the field's tail. `color_start` a dull ember, `color_end` cooler *and darker*, lifetime much longer than the flame's. Spawning smoke at the emitter base is the commonest mistake and it reads as two effects in the same place.

*Cost:* zero engine work if the offset is a child transform.

### 7. A back-layer glow quad instead of bloom

One large, very dim additive sprite behind the flame. Expressible **today** as a second `ParticleEmitter` with `additive = true`, large `size`, low `alpha` — zero engine work. Bloom is what conveys "brighter than the display can show" after the tonemap has compressed it away; ADR 0018 argued against a bloom pass and this recovers most of what it would buy without reopening that decision.

This is also the cheapest possible test of "does the fire need bloom".

### 8. Re-run the sprite-stack experiment against the HDR target — as a *measurement*, not a migration

The clipping argument at `scene.slang:2337` is void, and the flipbook path already exists and works: 8×8, 64 frames, 18 fps, per-particle seed, half-texel *cell* inset (`:2565-2586`), fed by `tools/texture/flipbook.py`. It is unreachable for flames only because `if (in.flame > 0.5)` returns first at `:2533` and the flipbook gate at `:2546` is never evaluated.

Three things to know before running it:

- **`uint(phase)` at `:2574` discards the fraction.** That is Epic's `Sub UV Blending Enabled` missing. At 18 fps it will strobe on close-ups. Add the lerp to the next cell first or you will measure the wrong thing.
- **`fireFlipbook` is one `uint` in the environment block** (`scene.slang:165`, `renderer.rs:289`). A flipbook fire is a **per-scene singleton**, not per-emitter. Fine for an experiment; a blocker for shipping two different fires in one scene.
- **You will lose the one-pixel-at-any-distance edge.** The level set's analytic `cover` is genuinely better than a bilinear atlas ramp, and it is currently 4× MSAA'd in the forward pass on top of that. Judge on `cargo xtask flythrough`, not on a still.

### Not recommended at any price: motion-vector flipbooks and TLRB directional lighting

Real, well-documented (RG = optical flow, B = emissive, A = alpha; a second texture carrying Top/Left/Right/Bottom baked lighting), and a content-pipeline investment aimed at hero explosions. It presumes a sprite architecture Loom has not committed to.

---

## 4. The architecture, if it is worth it

### 4.1 The honest verdict first

**None of this fixes the fire.** A parameter map, a module stack, a GPU sim and a stage scheduler all leave `scene.slang:2527` and the three `FIRE_*` constants exactly as they are. They would change zero pixels of `campfire`.

And there is a prior question nobody in the research answered: **which behaviour, in which scene, was actually blocked?** Twelve scenes author a `ParticleEmitter`; two author `flame = true`. If the answer is "none yet, but it will be," that is rung 1 of the ladder and the honest response is §3 plus the thirty-line check in stage 0 below.

That said, the staged path is real and each stage is independently shippable. Here it is, with what it costs.

### 4.2 Stage 0 — the validator, and do this whether or not you ever build a stack

**This is the cheapest correctness win in the whole report and it stands entirely on its own merit.**

Two holes, both read this session:

- **`TypeRegistry::validate` walks only the component's top level.** `crates/loom_reflect/src/lib.rs:155-172`: for a `Value::Array` field it takes `bounds(field_schema.get("items"))` and calls `element.as_f64()` per element. An element that is a *table* returns `None` and is skipped entirely; `$ref` into `$defs` is never resolved. The precedent already in the tree is `VoxelVolume.ops`, whose schema is literally `"items": true` — anything validates — and whose own doc comment records the resulting bug (`yaw = 45` written as degrees against code reading radians: a factor of 57, no error, a plausible render).
- **There is no general "does serde accept this component" check.** The only one that exists is hand-written and `WaterBody`-only: `crates/loom_scene/src/scene.rs:572,590-610` runs `check_water` and emits `component_unreadable` if `from_value::<WaterBody>` fails. Its own comment states the principle — *a value this layer does not understand is a value it must refuse, not one it may ignore.*

**Generalise `check_water` into "deserialize every registered component into its Rust type and report the serde error."** Roughly thirty lines. It closes `VoxelVolume.ops`, it closes the hand-rolled second reader in `crates/loom_cli/src/particles.rs` (which reads fields with `f(name, fallback)` closures rather than `from_value`, so an unknown key is silently ignored there too), and it is the exact defect class the S4 prefab lesson in CLAUDE.md already names: *a key it does not understand is a key it ignores.*

**Ship this today, decoupled from every particle proposal.**

### 4.3 Stage 1 — `SceneOp::SetField` addressing into a list

`crates/loom_scene/src/ops.rs:690` does `field.split_once('.')`, error text `"must be ComponentType.field"`. There is no index syntax.

So under any list-shaped component, an agent tuning one module's one parameter must rewrite the **entire array** as one JSON value, the editor's inspector has nothing to bind a slider to, and the git diff of a tuning change is the whole stack rather than one line. `VoxelVolume.ops` survives this because ops are authored wholesale. **A module stack is a thing you *tune*, which is different**, and this is why the "a git diff of adding a force is three lines" argument is true for *adding* and false for *tuning*.

`SetField` needs to accept `ParticleEmitter.update[2].strength`. Independently useful; independently green.

### 4.4 Stage 2 — the ordered stack, with a **closed** tagged-enum module list

Only after 0 and 1. The shape:

```toml
[node.components.ParticleEmitter]
seed = 7
lifetime = 3.0
lifetime_jitter = 0.35

  [[node.components.ParticleEmitter.spawn]]
  module = "rate"
  per_second = 40.0

  [[node.components.ParticleEmitter.update]]
  module = "gravity"
  accel = 1.2

  [[node.components.ParticleEmitter.update]]
  module = "curl_noise"
  strength = 1.4
  scale = 0.35
```

Grounded arguments for exactly this:

- It is the `[[…ops]]` spelling the format already supports and the parser's finiteness walk already descends into (`loom_scene/src/lib.rs:317-337` — that descent was itself a bug fix, because a `nan` inside an op list validated clean).
- **Two named lists, not one.** Niagara's ordering rule is per-group and the group is what makes ordering comprehensible. Two is the smallest that expresses "once at birth" vs "every tick".
- Keys that are genuinely per-emitter (`seed`, `lifetime`, `additive`, `flame`) stay flat, because they are not modules and pretending they are is how a stack grows a `SetLifetime` module.
- **Order matters and must be in the description string**, the way `VoxelVolume`'s already says the list *"is ordered and NOT commutative … Said here because an agent will otherwise assume it can reorder freely."*
- A tagged enum, matched in one place, is both the ponytail answer and the `schemars` answer. A `trait ParticleModule` is banned by never-do #12 with one implementation, and a `Box<dyn>` on the CPU cannot generate GPU code anyway.

**The tightest constraint, and it is a CI rule:** `loom_scene` depends on nothing in the workspace, so the module enum must live *in* `loom_scene` for its schema to be derivable at all, with `loom_particles` and `loom_render` reading it. That is fine — but it means **every new module is a three-crate engine commit** (enum, CPU impl, Slang emit).

**Be honest about what that buys and what it does not.** You get ordering, multiplicity, spawn-as-a-module, and a readable diff. You do **not** get Niagara's headline benefit — behaviour as content, no engine change. Pass 1 ranked the open payload first and priced the closed version; those are different products and the plan should name which one is being bought.

**Nobody costed the migration and it is the expensive part.** The current force chain — swirl → gravity → wind → drag → Euler, `loom_particles/src/lib.rs:200-267` — is baked into **eight golden references**. "Order is authored" means every one of those scenes must be hand-authored back into the current order, or all eight move in one commit. A diff where eight references change at once is exactly the unreadable diff `MANIFEST.txt` exists to prevent. **Stage 2 does not ship without a named plan for those eight.**

### 4.5 Stage 3 — the open payload, and why I would not commit to it

This is the whole system, and the research pass that ranked it first said so in its own words: *"a sibling imperative language sharing the discipline, not the type."* Unpack what that commissions — a typed attribute namespace, a payload layout resolver, read-before-write validation, statements, assignment, branches, integer types, a CPU interpreter, a Slang emitter, a CPU/GPU agreement test, and a versioning story (Niagara has one because a module edit propagates into every scene that instances it).

For calibration: `loom_field` is 15 `Expr` variants producing one `float3 f(p, t)`, and it needed ADR 0006, a `build.rs` backend, a frozen-ABI noise hash and an agreement test to be trustworthy. The payload resolver alone is comparable; the imperative language is several times it.

**`Expr` cannot be grown into it, and the ADR already says why.** `Expr` is 15 variants over `(p, t)` with three float outputs, no assignment, no branches, no persistent state, no integers. A module needs struct-in/struct-out, per-particle mutable state, sequencing where module *k* reads what *k−1* wrote, control flow (`Kill Particles`, `Do Once`), and `uint` seeds. That is a small imperative language with a type system.

**The narrow, genuinely valuable use of `loom_field` here:** a *force* module whose body is a pure `float3 f(p, t)` — wind, curl noise, a vortex, a radial force — **is** a `Field`. `loom_field::wind` already is one, `field_agree.rs` already dispatches one from compute, and `rain_sim.slang` already `#include`s the generated header. So: forces through `loom_field` and get CPU/GPU agreement free; anything with state or branching hand-written in Slang. That split matches what ADR 0006 already says the tool is for.

**Revisit stage 3 when there is a list of effects an agent actually failed to author.** Today the evidence for it is architectural taste.

### 4.6 GPU-resident particle simulation — blocked, and not on cost

The cost case dissolves on its own numbers. From the research pass's `LOOM_GPU_TIMING=1` runs on this box (RTX 4090 at 300 W, `target/release/loom`, 1920×1080) — **treat these as that pass's measurements, not mine**: `smoke --sim 300` forward 0.227 ms with 725 particles; CPU side ~25 ns per particle-tick (±30%, a difference of process timings, not an in-process profiler). 800 particles is ~0.02 ms/tick, about 3% of `smoke`'s forward pass. `MAX_PARTICLES = 65536` (verified, `renderer.rs:994`), so the buffer's own ceiling is roughly an order of magnitude away.

But there is a harder blocker that none of the passes stated, and it is structural:

**A GPU particle system with spawning cannot pass `cargo xtask image`, and rain does not prove otherwise.** Rain meets byte-identical-across-processes because thread *i* touches only `drops[i]`, the population is *fixed*, seeding is a pure function `rainSeed(index, eye, wind)`, and landed drops are recycled **in place**. It never spawns. A real particle system's spawn needs an atomic counter; the atomic's ordering picks the slot; the slot picks the seed; the seed picks position, roll and phase — all visible in the golden image. The only spawn scheme that survives is "slot = pure function of (index, tick)", i.e. a fixed pre-seeded pool, which *is* rain, which already exists.

Two further gaps the rain precedent does not cover: **there is no GPU sort**, and Loom's particles are premultiplied-alpha and CPU-sorted back-to-front (`renderer.rs:1625`) — a GPU-resident buffer cannot be CPU-sorted, so you either add a bitonic/radix sort (~log²n dispatches, all of which are render-graph barriers by never-do #4) or constrain everything to additive. And `RainSimPush` is one 120-byte push block for one global effect; N emitters need a per-emitter parameter buffer indexed by particle.

**Defer until a scene needs more particles than the CPU can afford.** And note what it would buy even then: more particles, not a better-looking fire.

### 4.7 The line to write into whatever ADR eventually lands

**A particle that can be queried by a script or an assertion is simulated on the CPU; a particle that only draws may be simulated on the GPU.** That is not a compromise — it is Niagara's Sim Target enum by another name, and Loom already occupies it: `loom_rain::splashes` is kept as *"the CPU answer for anything that needs to reason about impacts without a GPU."*

Verified, not assumed: particles are already outside the sim hash. `state_hash` is `self.physics.state_hash()` (`crates/loom_cli/src/play.rs:973` → `crates/loom_physics/src/lib.rs:1067`), rigid-body bit patterns in handle order, and `loom_particles` appears in exactly one other `Cargo.toml` (`crates/loom_cli/Cargo.toml:18`). So the binding gate on particles is `cargo xtask image`, not the hash — and a GPU float entering the hash would make it driver-dependent anyway (ADR 0006's agreement test asserts an epsilon, worst 4.5e-5 over 512 samples, precisely because a GPU `sin` is not libm's).

---

## 5. Water

**The honest next step is shading, not simulation.** Loom's surface is already ahead of a default UE water body in several respects: Snell's window via one `refract`, caustics sampled at both bed and surface underside, foam that thresholds so a glassy sea is foamless, downwind foam drift, swash, shoaling attenuation from a real depth grid, and buoyancy driving a rigid body in `water_crate.loom`.

In order of visible impact:

1. **Screen-space refraction distortion.** `scene.slang:3853` already says *"Unrefracted UV. The distortion offset is the next slice."* It is the cheapest remaining win, it is already scoped in the source, and it needs no new pass.
2. **Reflections.** Whatever form; a scope decision of its own.
3. **Interaction ripples from a shallow-water heightfield** — the real gap, and the one thing a player *feels*. UE's answer is a 2D shallow-water solver in Niagara; The Chinese Room's *Still Wakes the Deep* implementation is the concrete recipe — capsule "pumps" on the character's arms, hands and feet inject velocity into the sim, and **GPU readback** then lets physics objects respond to the surface ([80.lv](https://80.lv/articles/learn-how-still-wakes-the-deep-used-unreal-engine-5-to-create-water-mechanics)).

**On (3), the researchers disagreed and I side with the critique, with one refinement.** One pass called it cheap — "one 2D texture, ping-ponged, in a compute pass" — and technically that is right; Loom has the machinery (`rain_sim.slang` is a stateful compute pass with render-graph-owned buffer barriers, and `loom_rain::collide` already bakes the collision world into a 3D image). But the reference implementation's value comes from the readback, and **the readback is what turns a shading problem into a determinism problem**: it violates `rain_sim.slang:35`'s stated structural rule (*"Nothing here is read back, ever"*), puts a device→host sync inside the fixed step, and feeds gameplay.

So: **if ripples are ever built, build them rendering-only — no readback, no buoyancy coupling — and give them their own ADR.** Not as a rider on a fire fix.

**What water does not need is a fluid solver.** Epic's own guidance puts 3D FLIP in the cinematics bucket and ships Fluids with a GPU-crash warning. Adding one would be building the thing Epic says is for cinematics to fix a problem that is 90% shading.

---

## 6. Rejected

| Proposal | Killed by |
|---|---|
| **The open payload / parameter map as the first thing built** | Ladder rung 1 + scope. It is a small imperative language with a type system, two backends and a versioning story — several times `loom_field`. And under the CI rule that `loom_scene` depends on nothing, the *cheap* version does not deliver the benefit the expensive version is sold on (behaviour as content). Revisit when a real authoring task has failed. |
| **GPU-resident particle simulation** | Determinism, not cost. Spawning needs an atomic; the atomic picks the slot; the slot picks the seed; the seed is in the golden image. Rain passes the gate *because it never spawns*. Separately, no GPU sort exists and premultiplied-alpha particles require one. |
| **Simulation Stages** | Worth nothing until a module vocabulary exists to put in one. **And specifically reject folding GPU grass placement into the same ADR** — CLAUDE.md deferred that on a measurement (0.054 ms for 45,460 blades); do not un-defer a measured decision by attaching it to something unbuilt. |
| **Niagara Fluids / any 3D grid or FLIP solver** | Epic ships it with a GPU-crash warning and calls 3D a cinematics feature. It would sit outside the sim hash permanently. Out of scope for a solo engine. |
| **Events and event handlers** | CPU-only in Niagara itself, requires persistent IDs, and Loom already has a deterministic event log with damage on it doing the gameplay half. The one useful narrow case — a GPU collision spawning particles elsewhere — already works in `rain_sim.slang`; only its authorability is missing. |
| **Data Interfaces as a named abstraction** | Never-do #12 in spirit. `loom_voxel::exposure` and `loom_rain::collide` already *are* this; naming the pattern before a third caller exists is an interface with one implementation. **Keep the boundary as a written rule** — world-state queries stay out of the module vocabulary, and the S3 rule that the voxel march and audio's `openness` stay separate is the part worth a paragraph. |
| **Multiple renderers per emitter / a Light Renderer as architecture** | One pipeline already draws additive and alpha via premultiply and two sign bits (`renderer.rs:94-104`, `scene.slang:86-93`). `Light` already exists and `campfire` already proves it lights the scene at `intensity = 3.0`. Do not import a renderer list to obtain a sine wave — §3 item 4 is one scalar. |
| **A visual node-graph editor** | Niagara has one because artists were the users. Loom's users are an LLM and one developer, both of whom read text faster than graphs. Take the Scratch Pad's *idea* — a module authored locally, promotable to a shared asset — and note that S4 prefabs (`[[prefab]]`, `extends`, `[node.overrides]`, ADR 0008) already deliver Niagara's emitter-inheritance-and-versioning story. That deletes a whole subsystem from any proposal that reaches for it. |
| **Motion-vector flipbooks / TLRB six-way lighting** | A content pipeline for hero explosions, presuming a sprite architecture Loom has not committed to. |
| **Shallow-water heightfield with GPU readback** | The readback. See §5. The rendering-only half is arguable later, on its own ADR. |
| **Bloom, to make the fire read brighter** | ADR 0018 argued it out. §3 item 7 — one dim back-layer additive quad, authorable today with zero engine work — recovers most of what it conveys without reopening the decision. |

---

## 7. What I could not determine

**About Niagara.** Each of these is settled by reading a primary source I did not find this session, not by an experiment:

- The exact spellings of the module-internal namespaces (`Module.`, `Local.`, `Output.`, `Transient.`, `StackContext`). The *concept* — a module's declared inputs live in their own namespace, which is what makes them per-instance stack widgets — is certain. **Settled by:** finding Epic's Parameters Panel / Script Editor reference and reading the namespace list verbatim.
- Whether the compiler strips attributes written but never read from the payload. Matters because it determines whether a Loom port's layout resolver needs a liveness pass. **Settled by:** the same doc search, or by inspecting an Attributes Spreadsheet in a real editor.
- The VectorVM bytecode format. Name confirmed (Epic public roadmap); internals not publicly documented that I found. Probably irrelevant — Loom would emit Rust, not bytecode.
- Any published GPU-cost figure for Niagara Fluids 2D vs 3D gas. The "when to use which" boundary in §1.10 is Epic's qualitative guidance only.
- **Niagara Data Channels** — a newer cross-system communication feature I did not research at all and cannot describe.
- I confirmed there is **no Volume Renderer** on Epic's Render Module Reference page. I did not exhaustively confirm one does not exist elsewhere.

**About Loom.** Each of these is an experiment:

- **Do §3 items 1–3 fix `lanternhead`'s deck?** I believe so and I cannot prove it. **Settled by:** apply 1–3 in one commit, render `lanternhead` against the open deck (not against the dark headland it was repositioned onto), and run `cargo xtask flythrough` — a still cannot see whether the tongues cohere in motion, which is the actual question.
- **Does the raymarch (item 5) beat items 2+4?** Genuinely unknown; the passes disagreed on whether it is necessary. **Settled by:** ship 1–3 first, measure, *then* the raymarch as a separate commit. Never both in one — two tools measured as one is not a measurement.
- **Is the sprite stack now competitive against the HDR target?** The 8-bit argument that killed it is void, and nobody has re-run it. **Settled by:** unreachable-branch flip at `scene.slang:2533`, sub-image blend added first, 15–30 sprites with per-particle seed and rotation, judged on `flythrough` against the level set. Note the per-scene-singleton limitation of `fireFlipbook` before treating a good result as shippable.
- **Would a wider blade—sorry, would a wider `cover` band help at all?** I claimed in §2.6 that softening the level-set edge converges back on a fireball. That is inference from the shader's own comments, not a measurement. **Settled by:** one render with `cover` widened, compared against 1–3.
- **Does `decide_buffer` in the render graph handle a long read-write chain on one buffer efficiently?** Relevant only if a GPU sort is ever built (a bitonic sort is ~log²n dispatches). I did not read that code. **Settled by:** reading `crates/loom_render_graph` and, if it matters, a synthetic multi-pass test.
- **How much of the eight-reference migration in §4.4 is genuinely unavoidable?** It may be that the current force order is expressible as a fixed default module list, making the migration a no-op diff. **Settled by:** write the default stack for `smoke.loom` and check whether its reference moves at all.

---

*Sources for §1, all fetched or cited this session:* [Key Concepts in Niagara Effects](https://dev.epicgames.com/documentation/en-us/unreal-engine/key-concepts-in-niagara-effects-for-unreal-engine) · [Render Module Reference](https://dev.epicgames.com/documentation/en-us/unreal-engine/render-module-reference-for-niagara-effects-in-unreal-engine) · [Overview of Niagara Effects](https://dev.epicgames.com/documentation/en-us/unreal-engine/overview-of-niagara-effects-for-unreal-engine) · [Particle Update Group Reference](https://dev.epicgames.com/documentation/en-us/unreal-engine/particle-update-group-reference-for-niagara-effects-in-unreal-engine) · [Emitter Update Group Reference](https://dev.epicgames.com/documentation/en-us/unreal-engine/emitter-update-group-reference-for-niagara-effects-in-unreal-engine) · [Niagara Scratch Pad Modules](https://dev.epicgames.com/documentation/unreal-engine/niagara-scratch-pad-modules-in-unreal-engine) · [Events and Event Handlers Overview](https://docs.unrealengine.com/4.26/en-US/RenderingAndGraphics/Niagara/EventHandlerOverview) · [UNiagaraSimulationStageGeneric::Iterations](https://docs.unrealengine.com/5.2/en-US/API/Plugins/Niagara/UNiagaraSimulationStageGeneric/Iterations/) · [Niagara Fluids](https://dev.epicgames.com/documentation/en-us/unreal-engine/niagara-fluids-in-unreal-engine) · [Fluid Simulation Overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/fluid-simulation-in-unreal-engine---overview) · [Niagara Flipbook Baker Quick Start](https://dev.epicgames.com/documentation/unreal-engine/niagara-flipbook-baker-quick-start-guide-in-unreal-engine) · [Scalability and Best Practices for Niagara](https://dev.epicgames.com/documentation/en-us/unreal-engine/scalability-and-best-practices-for-niagara) · [Versioning Modules and Emitters](https://dev.epicgames.com/documentation/unreal-engine/versioning-modules-and-emitters-in-niagara-effects-for-unreal-engine) · [Sparse Volume Textures](https://dev.epicgames.com/documentation/unreal-engine/sparse-volume-textures-in-unreal-engine) · [Simulation Stages, Grid2D and GPU-driven effects — StraySpark](https://www.strayspark.studio/blog/niagara-vfx-advanced-simulation-stages) · [Niagara — CPU VectorVM, Epic public roadmap](https://portal.productboard.com/epicgames/1-unreal-engine-public-roadmap/c/1486-niagara-cpu-vectorvm-experimental) · [80.lv — Still Wakes the Deep water mechanics](https://80.lv/articles/learn-how-still-wakes-the-deep-used-unreal-engine-5-to-create-water-mechanics)

*No repository files were read-modified or created. Everything above came from reads of the working tree at `62f9ebe`.*