// One lit box per scene object, drawn in a single instanced draw call.
//
// Both the geometry and the per-object data are reached through **buffer
// device address** — the scene is a pointer graph the GPU walks, with no
// descriptor sets and no vertex input state (CLAUDE.md locked decisions).
//
// Push constants carry only two pointers, so adding per-object fields never
// runs into the 128-byte push limit: the object array grows instead. CPU work
// per frame is one draw call regardless of object count, which is the property
// §B.1 of the graphics doc is after.

// A vertex in 8 bytes, not 32.
//
// Blocky-voxel renderers pack a vertex into one 32-bit int because corners sit
// on integer lattice points and normals are one of six axis directions. Smooth
// SDF voxels have neither property — Surface Nets puts a vertex anywhere in its
// cell with an arbitrary normal — so the same idea needs different arithmetic:
// quantise against the mesh's own bounds, and encode the normal octahedrally.
//
// Both members are 4-byte, and the struct is 8, which still satisfies std430
// for a PhysicalStorageBuffer block: the requirement is that each member is
// aligned to its own size, not to 16.
// The water surface, generated from `loom_water` by build.rs. Included up here
// rather than beside the fields because `EnvironmentData` below embeds a
// `LoomWaveSet`, and a struct cannot contain a type declared after it.
#include "generated/water.slang"

struct Vertex {
    uint packedPosition;   // 10 bits per axis, 2 spare
    uint packedNormal;     // octahedral, 16 bits per component
    uint packedUv;         // 16 bits per component, against the mesh's UV bounds
};

struct ObjectData {
    float4x4 mvp;
    // The model matrix. Field order here mirrors the Rust `ObjectData` exactly;
    // the two are one memory layout described twice, and a mismatch is silent.
    float4x4 model;
    // Decode parameters for this object's packed vertices: xyz origin, w step.
    // One per object rather than per vertex, which is what makes the
    // compression free rather than moving the cost somewhere else.
    float4 unpack;
    // Decode parameters for this object's packed UVs: xy origin, z step.
    float4 uvUnpack;
    // Rows of inverse-transpose(model)'s upper 3x3. Non-uniform scale is
    // everywhere in a blockout, and it skews normals unless corrected.
    float4 normalRow0;
    float4 normalRow1;
    float4 normalRow2;
    float4 color;
    // Material index in x. `NO_MATERIAL` means "use color directly", which is
    // what keeps an untextured blockout readable.
    //
    // **y is this object's mesh's first index**, which is what turns a ray hit
    // back into a triangle: `RayQuery` reports `CommittedPrimitiveIndex`
    // counted from the start of the BLAS's build range, and each mesh's range
    // begins at `y` in the shared index buffer. There is no first-*vertex*
    // term — indices are absolute into `push.vertices`.
    uint2 material;
    // The shared index buffer. `uint2` and a pointer are both 8-aligned, so
    // this and the field above occupy exactly the `uint4` that used to sit
    // here and the record is still 240 bytes. Read only by
    // `tracedEnvironment`; the draw itself binds the same buffer as an index
    // buffer, because index fetch is fixed-function and cannot go by pointer.
    uint* indices;
    // The travelling body wave — ADR 0062. x amplitude (m), y wavenumber
    // (waves per body length), z frequency (Hz), w phase (turns). **x <= 0 is
    // the rigid early-out**, the same shape `windBend` uses for `sway <= 0`,
    // and it is what keeps every scene authored before `Deform` rendering
    // byte-identically rather than merely within tolerance.
    float4 deform;
    // The frame that wave is measured in: x the model-space nose coordinate,
    // y the SIGNED reciprocal of the body length along the nose axis, z the
    // body fraction the envelope starts at, w an axis code
    // `nose_axis * 4 + beat_axis` with 0 = x, 1 = y, 2 = z.
    float4 deformFrame;
};

static const uint NO_TEXTURE = 0xFFFFFFFF;
static const uint FLAG_TRIPLANAR = 1;

// Mirrors `loom_render::MaterialData` exactly. One memory layout described
// twice; a mismatch is silent and looks like a shading bug.
struct MaterialData {
    float4 albedo;   // rgb base colour, w porosity (how much it darkens wet)
    float4 params;   // metallic, roughness, uvScale.x, uvScale.y
    uint4 maps;      // albedo index, normal index, flags, layer index
    // rgb the material's AVERAGE linear albedo — the authored `albedo` times
    // the mean colour of its albedo texture, reduced once at load.
    // Read by `tracedEnvironment`, which has no UVs; see the comment there.
    // w is the ALPHA-TEST THRESHOLD: discard a texel below it, 0 = opaque.
    float4 meanAlbedo;
    // x is OPACITY: 1 opaque, less blends. yzw free.
    float4 misc;
};

static const uint LOOM_MAX_LIGHTS = 8;

/// The fire flipbook's shape. 8x8 frames played at 18 a second, which is fast
/// enough that a flame flickers rather than pulses and slow enough that the
/// 64-frame loop lasts three and a half seconds before repeating.
static const int FIRE_FLIPBOOK_GRID = 8;
static const int FIRE_FLIPBOOK_FRAMES = 64;
static const float FIRE_FLIPBOOK_FPS = 18.0;

/// A point light. Mirrors `loom_render::PointLight` exactly — one memory
/// layout described twice, and a mismatch is silent and looks like a shading
/// bug.
struct LoomPointLight {
    /// xyz world position, w luminous intensity.
    float4 position;
    /// rgb colour, w the radius past which it contributes nothing.
    float4 color;
};

struct ParticleInstance {
    // xyz world, w radius. A NEGATIVE radius marks an additive particle —
    // fire, a spark, anything that emits light rather than blocking it. A
    // radius is never legitimately negative, so the sign is free to carry one
    // bit and no third field was needed for it.
    float4 position;
    float4 color;      // rgb linear, a opacity
    // xyz world velocity in m/s, w the shutter time in seconds. **Zero shutter
    // is a disc**, which is every particle authored before this field existed:
    // a shutter open for no time records no smear, so the physical reading and
    // the compatibility reading are one number.
    //
    // Mirrors `loom_render::ParticleInstance`. All three are `float4` because
    // std430 aligns a `vec3` to 16 anyway — a three-float member here would
    // put a hole in one side of the layout and not the other.
    float4 velocity;
};

// The sky, the sun and the haze, as the scene sets them. These were `static
// const` — one blue afternoon baked into every scene — so a scene could place
// lights but could not turn the sun down, and night was not expressible.
struct EnvironmentData {
    float4 sun;        // xyz direction, w strength
    float4 sunColor;   // rgb, a ambient strength
    float4 zenith;     // rgb sky overhead, a fog density
    float4 horizon;    // rgb sky at the horizon, a fog falloff
    // Camera position. **Here rather than in the push block**, which was at
    // exactly its 128-byte guarantee before grass needed a pointer. This is
    // per-frame data read by whatever wants it, which is what this buffer is.
    float4 eye;
    float4 viewport;   // xy size in pixels, z the AO ray dial (`aoRays`), w the LOOM_ABLATE mask
    float4 wind;       // xy direction, z speed, w gustiness
    // x turbulence, y ground drag, z seconds, w the wetness FILM — see the
    // wetness section further down, and `water.w` for its other half.
    float4 weather;
    // **What the SKY is doing, unsheltered.** xyz is the P1 wind a streak leans
    // along, w the rate in mm/h falling on the scene. It used to be
    // `loom_rain::sample_rain` at the EYE — exposure-scaled where the camera
    // stands — and the layer then switched itself off whenever the camera
    // walked under a roof, taking the rain in the open with it. Occlusion is
    // per drop, below, against the terrain height field. The GPU layer derives
    // everything else from these four floats and writes nothing back.
    float4 rain;
    // x still-water level, y 1 when the eye is submerged, z 1 when the scene
    // has water, w the wetness SOAK.
    // Depth used to sit in `y` as a constant; it is a real per-vertex query
    // now, out of the height field below.
    float4 water;
    // The terrain height grid: xy world origin, z metres between samples,
    // w samples per axis. **Zero samples means no terrain**, and every depth
    // query is then bottomless — which is what an open ocean wants.
    //
    // Split from the pointer that follows only because `LoomHeightField` puts
    // its pointer last and a float4 must stay 16-byte aligned; the two are
    // assembled back into one in `waterVertexMain`.
    float4 terrain;
    // The heights, `terrain.w²` of them, row-major. **A pointer in this buffer
    // rather than in the push block**, which is at 124 of its 128 bytes.
    float* terrainHeights;
    // This frame's raindrops and splashes, as `rain_sim.slang` left them. Null
    // in a scene with no rain, and the draws are skipped then.
    //
    // **Here rather than in the push block**, which is at 124 of its 128 bytes;
    // and immediately after the other pointer, so both sit at an offset a
    // `float4` would have to pad to reach.
    RainDrop* rainDrops;
    RainSplash* rainSplashes;
    // **The sea itself, in the environment buffer rather than behind another
    // pointer.** The push block is at 116 of the 128 bytes Vulkan guarantees;
    // this is per-scene data read once per vertex, which is exactly what this
    // buffer is for.
    LoomWaveSet waves;
    // How far the camera moved in the last `RAIN_TICK`, in metres — xyz, w
    // unused. **Metres per tick, not per second**, so the timestep lives only
    // in this file; `rainVertexMain` divides by `RAIN_TICK` itself.
    //
    // After the wave table because every offset above it is pinned by a test
    // against this struct's Slang layout, and a `float4` inserted higher up
    // would move `waves` and pad besides.
    float4 eyeStep;
    // x cloud cover 0..1, y metres across a cloud mass, z drift multiplier on
    // the wind, w unused. Feeds the generated `clouds_at`; the cover here is
    // already the effective one, with rain's floor applied on the CPU.
    float4 cloud;
    /// Point lights. Last in the struct so every offset above is unmoved.
    LoomPointLight lights[LOOM_MAX_LIGHTS];
    uint lightCount;
    /// Bindless index of the fire flipbook, or `NO_TEXTURE`.
    uint fireFlipbook;
    /// The tonemap's exposure. **Declared here and read by nothing in this
    /// file** — `tonemap.slang` takes it as a push constant — but the layout
    /// has to match the Rust struct byte for byte, and a field the CPU writes
    /// and this side calls padding is how that agreement stops being checkable.
    float exposure;
    uint lightPad1;
    /// The interactive wavelet events: x how many there are, y unused, zw
    /// unused. **Zero events is water nothing has touched**, and every lookup
    /// then returns zero — the plain Gerstner surface, bit for bit.
    float4 wavelet;
    /// The events, `wavelet.x` of them. **A copy of the CPU pool, which is the
    /// authoritative one** — nothing here ever writes back, because a GPU
    /// float reaching a force is what ADR 0045 clause 2 forbids.
    LoomWaveletEvent* waveletEvents;
    /// Keeps the struct's stride 16-byte aligned on both sides. `environment`
    /// is an array in the push block, so the stride has to match even though
    /// only element zero is read.
    uint2 waveletPad;
    /// The river's current: xy world origin, z metres between nodes, w nodes
    /// per axis. **Under two nodes means this water has no current**, which is
    /// every scene that does not author `WaterBody.flow`.
    float4 flow;
    /// The velocities, `flow.w²` of them, `(x, z)` in m/s. A copy of the bake
    /// `loom_water::flow` derives from the bed; nothing here writes it, and
    /// what it drives is texture rather than force.
    float2* flowVelocity;
    uint2 flowPad;
    /// The advected foam field: xy world origin, z metres between samples,
    /// w samples per axis. **Zero samples means this water has no field**, and
    /// the foam is then the closed-form trail alone, bit for bit as it was.
    float4 foam;
    /// The coverages, `foam.w²` of them, in [0, 1]. A copy of the CPU field
    /// (ADR 0055), which is the authoritative one — this side never writes it,
    /// and `water@x,z.foam` reads the same numbers through the Rust half.
    float* foamCoverage;
    /// How many cells the boundary fade covers. Uploaded rather than compiled
    /// in, so `loom_water::foam::FOAM_EDGE_CELLS` cannot drift away from it.
    float foamEdgeCells;
    uint foamPad;
    /// The waterfall's lip, end A: xyz world, w the discharge in m²/s.
    /// **Zero discharge means this scene has no cascade** (ADR 0054), and the
    /// nappe's slice of the water draw then collapses to a point.
    float4 cascadeA;
    /// The lip, end B: xyz world, w `δ_out` — how fast the outside of the sheet
    /// spreads, per metre fallen.
    float4 cascadeB;
    /// xy the horizontal direction the water leaves along, z `δ_in` — the core
    /// spread — w how far the sheet falls before it reaches the pool.
    float4 cascadeC;
    /// The cinematic free surface's vertices — ADR 0057 addendum. Null on every
    /// scene outside the tier, and the draw that reads it is skipped from the
    /// CPU, so no other scene's shader touches this at all.
    LoomFluidVertex* fluidVertices;
    uint2 fluidPad;
    /// The FFT ocean — ADR 0076. x how many cascades, y cells per side, zw
    /// unused. **Zero cascades means this water is not a spectrum sea**, and
    /// `waterVertexMain` then sums the sixteen Gerstner waves exactly as it
    /// always did, which is every scene in the repository but `ocean_fft`.
    ///
    /// Appended after the fluid pointer for the reason that was appended after
    /// the cascade block: every offset above it is unmoved.
    float4 ocean;
    /// Each cascade's patch, in metres — one lane per cascade, cascade order.
    float4 oceanPatch;
    /// Each cascade's longest carried wavelength, in metres. **The Nyquist fade
    /// reads this and nothing else** — see `waterCascadeFade`.
    float4 oceanLongest;
    /// The tiles: `LOOM_OCEAN_TILES` of `ocean.y²` per cascade, in cascade
    /// order. A copy of the CPU cascade, which is the authoritative one — this
    /// side never writes it, and `loom water --at` reads the same numbers
    /// through the Rust half.
    float* oceanTiles;
    uint2 oceanPad;
    /// What this water is made of, optically: diffuse attenuation per metre at
    /// 650 / 550 / 450 nm — xyz, w unused.
    ///
    /// **This and `waterBackscatter` are the sea's colour**, and nothing here
    /// carries the colour itself; see `waterDeep()`. A scene that authors no
    /// `WaterBody.optics` uploads pure water — the values this file held as
    /// `WATER_EXTINCT` and `WATER_BACKSCATTER` — so it shades exactly as it did.
    ///
    /// Appended after the ocean block for the reason that block was appended
    /// after the fluid one: every offset above it is unmoved.
    float4 waterAttenuation;
    /// Backscatter per metre at the same three wavelengths — xyz, w unused.
    float4 waterBackscatter;
    /// The sea's own vertical scale: x is σ, the RMS surface elevation in
    /// metres, yzw unused. **Zero is glass** — a body with no wave set.
    ///
    /// `Hs = 4σ`, so `2σ` is how far a significant crest stands above still
    /// water; `WATER_CREST_SIGMA` is that two and `waterCrestScale()` is the
    /// product. Nothing else reads this.
    ///
    /// Appended after the optics pair for the reason that pair was appended
    /// after the ocean block: every offset above it is unmoved.
    float4 waterScale;
};

/// One vertex of the cinematic free surface, as `loom_render::FluidVertex`
/// packs it. Two `float4`s: std430 aligns a `float3` to 16 anyway, so the
/// natural six-float layout would be a hole on one side and not the other.
struct LoomFluidVertex {
    /// xyz world, w metres of water under this point.
    float4 position;
    /// xyz outward normal, w whitewater coverage.
    float4 normal;
};

struct GrassBlade {
    float4 position;  // xyz base, w height
    float4 facing;    // xy facing, z width, w rest tilt
    float4 shape;     // x bend, y shade, z clump hash, w clump albedo, 8:8:8
};

struct Push {
    // **First, and deliberately.** A `float4x4` needs 16-byte alignment; put
    // it after the pointers and it needs padding, and a `uint3` pad does not
    // do what it looks like — vectors align to 16 too, so the matrix lands at
    // offset 48 while the Rust mirror puts it at 32. Everything then reads
    // garbage and the sky comes out as diagonal stripes. Leading with the
    // matrix means every field is naturally aligned and there is no padding
    // to get wrong.
    //
    // NDC to world, for the sky's per-pixel ray. The scene entry points
    // ignore it; both pipelines share this block so they share one layout.
    float4x4 invViewProj;
    Vertex* vertices;
    ObjectData* objects;
    // The material table. A pointer rather than a descriptor: materials are
    // read by index and never bound, so adding one costs no API calls.
    MaterialData* materials;
    // This frame's particles, sorted back to front by the CPU. Null when the
    // scene has none.
    //
    // **Order matters and is checked by nothing.** This block is one memory
    // layout described twice, in Rust and here, and the compiler sees neither
    // half of the pair. Declaring this before `materials` — as it briefly was
    // — makes the fragment shader read the particle buffer as materials: the
    // ground turns black and a stray quad covers a third of the screen, with
    // no validation message anywhere.
    ParticleInstance* particles;
    // This frame's grass blades. The block is at 116 of its 128 bytes with
    // this — 64 + 48 + 4, padded to 120 — which is why the wind parameters live
    // in the environment buffer. It is *not* full: one more pointer fits, at
    // 124 padded to exactly 128. This line used to say 124 and ADR 0021
    // deferred a whole feature on it; the Rust half was corrected and this half
    // was not, which is the hazard of a layout described twice.
    GrassBlade* grass;
    // Index of this draw's first object.
    //
    // NOT derived from SV_InstanceID + firstInstance: HLSL's SV_InstanceID is
    // zero-based per draw and excludes StartInstanceLocation, while Vulkan's
    // InstanceIndex includes it. Which one Slang emits is a detail worth not
    // depending on, so the offset is passed explicitly and the arithmetic is
    // ours.
    // Before `objectOffset`, matching the Rust mirror: a pointer needs
    // eight-byte alignment and putting it last would pad the block to the
    // 128-byte limit exactly.
    EnvironmentData* environment;
    uint objectOffset;
};
// The analytic fields, generated from `loom_field` by build.rs. Included
// rather than written here: the CPU evaluates the same tree, and a hand-
// written copy is the divergence S2 exists to prevent.
#include "generated/fields.slang"
// The rain layer's state, shared with `rain_sim.slang` — which is a separate
// module, so anything both of them read has to live in a file both include.
#include "include/rain.slang"

[[vk::push_constant]] Push push;

// The sky, the sun and the haze. Read from the buffer rather than baked in as
// constants, so a scene can be at night. Accessors rather than globals because
// `push` is not in scope until here.
float3 sunDirection() { return normalize(push.environment[0].sun.xyz); }

/// How much of the sky is deck, for the light rather than for the picture.
///
/// The authored-and-floored scalar, not the per-ray sample: scene lighting
/// wants one number for the whole sky, and this is the one the deck was drawn
/// from, so the two cannot disagree about how overcast it is.
float cloudCover() { return saturate(push.environment[0].cloud.x); }

// **Cloud cover drives the light, and this is the half that makes a deck real.**
// Drawing clouds and leaving the sun at full strength gives a scene lit like a
// clear afternoon with a picture of clouds pasted above it — sun glinting off
// the water under a solid overcast. That is not a hypothetical: it is what the
// first version of this did, and it was spotted on sight.
//
// Overcast light is *dimmer, cooler and flatter*. The disc stops being a
// direction and the whole sky becomes the source, so direct light falls hard,
// its warm cast goes, and ambient rises to take over. Getting that wrong is
// more noticeable than the clouds themselves (ADR 0015).
//
// **Every lighting path in this file goes through these three**, which is why
// the coupling lives here and not at each site: specular highlights, wrapped
// foliage lighting, the sky gradient and the particle terms all follow without
// knowing clouds exist.
//
// Purely visual: ADR 0015 verified that nothing in `loom_script`, `loom_ecs` or
// `play.rs` reads sun strength or direction, so this cannot reach the
// determinism hash. If that ever changes, this coupling has to be revisited.
float sunStrength() {
    return push.environment[0].sun.w * lerp(1.0, 0.12, cloudCover());
}
float3 sunColor() {
    return lerp(push.environment[0].sunColor.rgb, float3(0.70, 0.75, 0.82),
                cloudCover() * 0.85);
}
float ambientStrength() {
    return push.environment[0].sunColor.a * lerp(1.0, 1.8, cloudCover());
}
/// **What a point light adds at a surface, and it is the thing that makes a
/// dark scene lightable.** Until this existed, `Light` was a component a scene
/// could author and nothing read — a knob wired to nothing — so a night scene
/// could be made black and then could not be lit back up by anything except
/// turning the sun on, which is not night.
///
/// Inverse-square falloff, windowed so it reaches exactly zero at the light's
/// radius. The window matters: a bare inverse square never reaches zero, so a
/// light either contributes everywhere — and every pixel pays for every light
/// — or is clipped at some distance and pops when it crosses it.
/// Ceiling on one light's contribution at one pixel.
///
/// Well inside fp16's 65,504 and far above any value that survives a tonemap,
/// so it changes no picture — it exists to stop an inverse square's
/// singularity from becoming an infinity the frame cannot recover from.
static const float LIGHT_MAX = 4096.0;

/// `minDist` is the radius of the thing being lit, and it exists because a
/// SPHERE is not a point.
///
/// A surface passes 1 cm and gets exactly the behaviour it always had. A smoke
/// puff passes its own radius, because a light *inside* a ball of smoke is at
/// zero distance from the ball's centre and `intensity / d^2` is then whatever
/// the clamp says — measured on the deck, a puff born 15 cm under the brazier's
/// light came out at sRGB (185, 143, 109), a hard white-hot bead floating over
/// the sea. The mean irradiance on a ball of radius R from a light at distance
/// d goes as `1 / max(d, R)^2`, and that is all this is.
float3 pointLights(float3 worldPos, float3 normal, float minDist) {
    float3 sum = float3(0.0, 0.0, 0.0);
    uint count = min(push.environment[0].lightCount, LOOM_MAX_LIGHTS);
    for (uint i = 0; i < count; ++i) {
        LoomPointLight l = push.environment[0].lights[i];
        float3 toLight = l.position.xyz - worldPos;
        float d2 = dot(toLight, toLight);
        float radius = max(l.color.w, 1e-3);
        if (d2 >= radius * radius) {
            continue;
        }
        float d = sqrt(max(d2, 1e-8));
        // Windowed inverse square: `1/d^2` shaped by `(1 - (d/r)^4)^2`, the
        // standard form. The squared window is what makes the derivative zero
        // at the boundary as well as the value, so the edge of a light's reach
        // is invisible rather than merely dim.
        float window = saturate(1.0 - pow(d / radius, 4.0));
        float atten = window * window / max(d2, minDist * minDist);
        float ndotl = max(dot(normal, toLight / d), 0.0);
        // **Clamped at the singularity, which is here and not in a later
        // pass.** The denominator is floored at 1 cm, so a surface that
        // touches a light's centre gets `intensity / 1e-4` — 300,000 at
        // intensity 30. Against an 8-bit fixed-point target that clamps
        // harmlessly to 1.0 on store and nobody notices; against a float
        // target it is `+inf`, the multisample resolve averages `(inf, x, x,
        // x)` to inf, and fog's `lerp` then computes `inf * 0` = **NaN** — a
        // pixel `loom compare`'s worst-channel test exists to catch, arriving
        // from an authoring mistake rather than a rendering one.
        //
        // A clamp in a tonemap would be too late twice over: after the resolve
        // has already spread the infinity, and because `clamp(NaN, ., .)` is
        // NaN.
        sum += min(l.color.rgb * (l.position.w * atten * ndotl), LIGHT_MAX);
    }
    return sum;
}

/// Surfaces, at the 1 cm floor this has always used. Byte-identical.
float3 pointLights(float3 worldPos, float3 normal) {
    return pointLights(worldPos, normal, 0.01);
}

float3 skyZenith() { return push.environment[0].zenith.rgb; }
float3 skyHorizon() { return push.environment[0].horizon.rgb; }
float fogDensity() { return push.environment[0].zenith.a; }
float fogFalloff() { return push.environment[0].horizon.a; }

/// Whether the eye is under the water. **Decided once, on the CPU.**
///
/// It is `loom_water::buoyancy::submersion_at` at the camera position — the
/// same query the buoyancy solver, the audio listener and `is_submerged(node)`
/// all ask, so a swimmer's ears and a swimmer's eyes cannot disagree about
/// which side of the surface they are on. Recomputing a wave height per pixel
/// to answer a question that is one bool per frame would also be a second
/// implementation of the thing this project keeps paying to keep single.
bool eyeUnderwater() { return push.environment[0].water.y > 0.5; }

/// The scene's still-water level, and whether it has water at all.
float stillWaterLevel() { return push.environment[0].water.x; }
bool waterPresent() { return push.environment[0].water.z > 0.5; }


// ---------------------------------------------------------------------------
// The sky.
//
// **One gradient, used twice**: as the background, and as the ambient term
// lighting the scene. Two independently-tuned constants is how a scene ends up
// lit by a sky it does not match — the shadowed side of a rock saying one
// thing about the weather and the horizon behind it saying another.
//
// The sun direction is shared for the same reason: the bright spot in the sky
// and the direction things are lit from are the same fact.
// ---------------------------------------------------------------------------

// **These are linear, and were converted rather than re-eyeballed.** They used
// to be the values that looked right on screen, which — with no sRGB encode on
// the way out — meant they were sRGB numbers being used as if they were linear.
// Fixing the encode made every one of them a gamma too bright. Each was passed
// through the sRGB-to-linear transfer function once, so the sky looks exactly
// as it did before while the arithmetic underneath it is now meaningful.

// Not near-black. Real ground bounces a great deal of light back up, and a
// downward-facing surface lit by a black "ground" reads as a silhouette — which
// defeats the point of a render the agent is supposed to be able to inspect.
static const float3 GROUND_COLOR = float3(0.0550, 0.0432, 0.0331);
/// How much of the sky's colour reaches a surface, and how much the sun adds.
/// Tuned together so an unlit face stays readable without washing out.

/// The sky's colour in a direction, without the sun.
///
/// Split from `skyColor` because the ambient term wants the gradient but not
/// the sun disc — a surface should not pick up a specular highlight from
/// pointing vaguely skyward.
// Enough to clear the surface a ray starts on without detaching contact
// shadows from what casts them.
static const float SHADOW_BIAS = 1e-3;

float3 skyGradient(float3 dir) {
    float height = clamp(dir.y, -1.0, 1.0);
    if (height >= 0.0) {
        // sqrt, not linear: the interesting part of a sky is near the horizon,
        // and a linear ramp spends most of its range on empty blue.
        return lerp(skyHorizon(), skyZenith(), sqrt(height));
    }
    return lerp(skyHorizon(), GROUND_COLOR, sqrt(-height));
}

/// What the background shows: the gradient, plus the sun.
/// Where a view ray meets the cloud deck, as a horizontal position.
///
/// The classic sky-plane projection: `dir.xz / dir.y` scaled by a deck height.
/// It compresses toward the horizon exactly the way a real deck does, and it is
/// why clouds have no parallax — see ADR 0015's list of what this cannot do.
///
/// **`dir.y` is floored rather than branched on.** At the horizon the quotient
/// runs to infinity, and an unbounded coordinate into a noise lattice is a
/// smeared band of garbage rather than a distant sky. The floor alone does not
/// fix that — it converts the infinity into a *stretch* — so `horizonFade`
/// below takes the deck out before the stretch becomes visible.
float3 cloudPlane(float3 dir) {
    const float DECK_HEIGHT = 900.0;
    float2 plane = dir.xz / max(dir.y, 0.10) * DECK_HEIGHT;
    return float3(plane.x, 0.0, plane.y) + push.environment[0].eye.xyz * float3(1, 0, 1);
}

/// How much of the deck survives at this elevation.
///
/// **Not a fudge.** A cloud deck really does disappear into haze toward the
/// horizon — you are looking through tens of kilometres of atmosphere at a
/// layer only a kilometre up — and the projection above is degenerate in the
/// same place. One term covers both, and without it the sky ends in a band of
/// vertical smears.
float horizonFade(float3 dir) {
    return smoothstep(0.02, 0.30, dir.y);
}

/// The deck along a view ray: x coverage 0..1, y raw density for shading.
///
/// **Two numbers because they answer different questions.** Coverage is opacity
/// and saturates by design; shading needs something that still varies when the
/// sky is solid, or an overcast day renders as a flat white ceiling.
/// How much cloud is over a *world position*, 0 to 1.
///
/// **The term that decides where it rains** (ADR 0016 step 3), and the same
/// `clouds_at` the sky is drawn from — one expression tree, one generated
/// function, so what you see overhead is what falls on you.
///
/// A position rather than a view ray: `cloudPlane` projects a direction onto
/// the deck, which is the right question for a pixel of sky and the wrong one
/// for a drop that is already somewhere.
float cloudCoverAtTime(float3 world, float t) {
    float4 cloud = push.environment[0].cloud;
    if (cloud.x >= 1.0) {
        // A solid deck rains everywhere. Skipping the evaluation here is what
        // keeps every scene that authors no cover costing exactly what it did.
        return 1.0;
    }
    if (cloud.x <= 0.0) {
        return 0.0;
    }
    float4 wind = push.environment[0].wind;
    CloudsParams params;
    params.cloud_cover = cloud.x;
    params.cloud_scale = cloud.y;
    params.cloud_drift = cloud.z;
    params.dir_x = wind.x;
    params.dir_z = wind.y;
    params.speed = wind.z;
    // **The time is a parameter because the deck advects along the wind**, so a
    // lagged `t` is where the deck *was*. The wetness average below is a walk
    // backwards through that.
    return clouds_at(float3(world.x, 0.0, world.z), t, params).x;
}

/// Cover over a point now. The same clock the grass bends on.
float cloudCoverAt(float3 world) {
    return cloudCoverAtTime(world, push.environment[0].weather.z);
}

float2 cloudSample(float3 dir) {
    float4 cloud = push.environment[0].cloud;
    if (cloud.x <= 0.0) {
        return float2(0.0, 0.0);
    }
    float4 wind = push.environment[0].wind;
    CloudsParams params;
    params.cloud_cover = cloud.x;
    params.cloud_scale = cloud.y;
    params.cloud_drift = cloud.z;
    params.dir_x = wind.x;
    params.dir_z = wind.y;
    params.speed = wind.z;
    // The same clock the grass bends on and the waves travel on.
    float3 got = clouds_at(cloudPlane(dir), push.environment[0].weather.z, params);
    return float2(got.x * horizonFade(dir), got.y);
}

float3 skyColor(float3 dir) {
    float3 base = skyGradient(dir);
    float toSun = max(dot(dir, sunDirection()), 0.0);
    // A tight disc inside a wide glow. Both are additive and modest — a sun
    // that blows out to white makes every screenshot harder to read, which is
    // the opposite of what this render path is for.
    float glow = pow(toSun, 8.0) * 0.18;
    float disc = pow(toSun, 512.0) * 0.9;

    float2 deck = cloudSample(dir);
    float cover = deck.x;
    if (cover <= 0.0) {
        return base + float3(1.0, 0.94, 0.82) * (glow + disc);
    }

    // **One extra tap toward the sun, and it is what makes clouds read as
    // clouds.** Sample the deck again a little way along the sun direction: if
    // it is *denser* that way, this patch is behind its own mass and goes grey;
    // if thinner, it is lit and goes white. Cheap fake self-shadowing, and the
    // highest realism-per-instruction trick available without raymarching a
    // volume (ADR 0015).
    //
    // **Density, not coverage.** Coverage saturates, so under an overcast sky
    // both taps read 1.0, every difference is zero and the deck shades to one
    // flat tone. That is not a hypothetical: it is what the first version of
    // this did.
    float3 sunward = normalize(dir + sunDirection() * 0.25);
    float shadow = cloudSample(sunward).y;
    float lit = saturate(0.5 - (shadow - deck.y) * 5.0);

    // Overcast grey through to a sunlit top. Deliberately not pure white: a
    // white that clips reads as a blown highlight rather than as cloud, and the
    // range is narrow because a real deck is a small spread of greys.
    const float3 CLOUD_DARK = float3(0.29, 0.30, 0.34);
    const float3 CLOUD_LIT = float3(0.78, 0.78, 0.79);
    float3 body = lerp(CLOUD_DARK, CLOUD_LIT, lit);

    // **The sun goes behind the cloud, rather than through it.** This is the
    // whole point of the coupling: rain falling out of a clear sky with a
    // blazing disc in it was the single biggest tell in any weather scene here.
    // The glow survives a little longer than the disc, because an overcast sky
    // really is brightest where the sun is behind it.
    float3 sun = float3(1.0, 0.94, 0.82) * (glow * (1.0 - cover * 0.75) + disc * (1.0 - cover));

    // Clouds sit in front of the gradient and behind the horizon haze, which
    // the existing fog term applies to everything.
    return lerp(base, body, cover) + sun;
}

/// Ambient light arriving at a surface facing `normal`.
///
/// A hemisphere: sky above, ground below, sampled from the same gradient the
/// background draws. This is what makes a shadowed face read as *outdoors*
/// rather than as a flat grey constant.
float3 hemisphereAmbient(float3 normal) {
    float up = normal.y * 0.5 + 0.5;
    return lerp(skyGradient(float3(0.0, -1.0, 0.0)), skyGradient(float3(0.0, 1.0, 0.0)), up);
}

// ---------------------------------------------------------------------------
// Underwater.
//
// **Not a pass.** ADR 0010 moved the "no post-process before Phase 8" boundary
// once, deliberately, to admit exactly one full-screen filter; a second one
// arriving as a detail of a water step is the drift that ADR exists to prevent.
// So being submerged is expressed entirely in things this file already does:
// the fog term every surface applies, the background the sky pass paints, and
// which side of the water surface is being shaded.
//
// That is not a compromise — it is what underwater physically is. Water is a
// participating medium with a short extinction distance and a strong colour,
// which is precisely what `fogAmount`/`fogColor` model. The only thing a
// screen-space pass would add here is distortion and caustics, and both are
// out of scope.
// ---------------------------------------------------------------------------

// **Both of the constants that used to be here are gone, and that is the whole
// of this section's change.** The extinction was one scalar per metre and the
// tint was a hand-picked teal, and between them they painted everything below
// the waterline — so `WaterBody.optics` reached the surface and stopped there.
// A scene could author Jerlov III, watch the sea above go turquoise, and swim
// into a frame that had never heard of it. `waterExtinct()` and `waterDeep()`
// are the same two accessors the surface reads; nothing new is plumbed.
//
// **`underwater.loom` is submerged and blessed and could not catch this**,
// because it authors no `optics` and a constant is the same picture as a
// correctly-read default. `ocean_under.loom` is the scene that can.

/// How much of the surface's light still reaches you looking straight DOWN.
/// Small: below you is a deep column that has already absorbed nearly
/// everything, which is why the sea floor of a photograph fades to black long
/// before the surface does.
///
/// **The last calibration knob in this section, and it is now a ratio rather
/// than a brightness.** `underwaterColor` anchors its downward limit on
/// `waterDeep()`, so this no longer scales the colour at all — it says only how
/// much brighter the ceiling is than the void. That ratio is a property of the
/// *surface* transmitting, not of the water absorbing, and nothing in
/// `WaterOptics` measures it: an honest constant, and named as one.
static const float UNDERWATER_DOWN = 0.13;
/// The sun through a rippled ceiling — a soft bloom, not a disc.
static const float UNDERWATER_SHAFT = 0.55;
static const float UNDERWATER_SHAFT_POWER = 6.0;
/// How hard the caustic web reads on the underside of the surface. Stronger
/// than on the bed: you are looking straight at the lens rather than at what
/// it has lit, with no water in between to wash it out.
static const float UNDERSIDE_CAUSTIC = 1.8;

/// What the water looks like in a direction with nothing in that direction.
///
/// Scaled by how much light is actually up there, so a night dive is dark and
/// an authored-dark environment stays dark. Brightness rather than colour is
/// taken from the sky — multiplying the veil by sky *colour* would drag the
/// hue back toward whatever hue the sky is, which is the one thing this is
/// meant not to do.
///
/// **The colour is `waterDeep()`, which is the surface's own `R∞`.** Looking
/// straight down from inside the medium into a column with no bed in it, what
/// reaches the eye is light that scattered back up out of that column — which
/// is exactly the quantity `R∞ = 0.33·b_b/(K_d + b_b)` names, and exactly the
/// term `waterFragmentMain` multiplies by `downwelling` at `L -> infinity`.
/// So the sea seen from a metre above the surface and the sea seen from a
/// metre below it are now two evaluations of one expression, and a scene
/// cannot author a turquoise sea that turns teal when you swim into it.
float3 underwaterColor(float3 dir) {
    float above = ambientStrength()
                * dot(skyGradient(float3(0.0, 1.0, 0.0)), float3(0.2126, 0.7152, 0.0722))
                + sunStrength() * 0.25;

    // **Which way you look matters, and it used to not.** This returned one
    // colour for every direction, which is a flat teal void with a hard line
    // where the seabed starts — no sense of a surface overhead, no sense of
    // depth below.
    //
    // All the light down here came through the surface, so it arrives from
    // above: looking up you see it near-undiminished, looking down you see
    // only what has scattered back out of a deep column, which is very little.
    // That gradient is most of what makes being submerged read as submerged.
    float up = saturate(dir.y * 0.5 + 0.5);
    float column = UNDERWATER_DOWN + (1.0 - UNDERWATER_DOWN) * up * up;

    // The sun, still visible as a soft bloom through a rippled ceiling. Only
    // upward — a sun glow on the seabed side would be light arriving from
    // where there is none.
    float shaft = pow(saturate(dot(dir, sunDirection())), UNDERWATER_SHAFT_POWER)
                * saturate(dir.y) * sunStrength();

    // **And it crossed the water to get here.** This added `sunColor()` — a
    // warm near-white — with nothing between it and the eye, which was
    // survivable only while the veil beside it was a bright hand-picked teal.
    // Against a derived `R∞` it is the loudest thing in the frame, and an
    // orange sun bloom three metres down is the one colour water guarantees you
    // cannot see: red is the first thing it takes. So the shaft gets the same
    // Beer-Lambert the seabed gets, over the slant from the surface to the eye
    // along the sun's own ray. The `0.15` floor on the sun's elevation is the
    // one `waterFragmentMain` already uses for the same division.
    float slant = max(stillWaterLevel() - push.environment[0].eye.y, 0.0)
                / max(sunDirection().y, 0.15);

    // **The downward limit is the derived one and the rest is a ratio to it.**
    // `column` was written as a fraction of an authored tint that was itself
    // the *upward* brightness; dividing by `UNDERWATER_DOWN` re-anchors it, so
    // `dir.y = -1` gives `waterDeep() * above` on the nose — the veiling light
    // of a semi-infinite column, no constant involved — and looking up scales
    // that by `1 / UNDERWATER_DOWN`. **`UNDERWATER_DOWN` is therefore still a
    // calibration knob and is now the only one left here**: it says how much
    // brighter the ceiling is than the void, which is a real ratio nothing in
    // `WaterOptics` measures, because it depends on the surface transmitting
    // rather than on the water absorbing.
    return waterDeep() * above * (column / UNDERWATER_DOWN)
         + sunColor() * exp(-waterExtinct() * slant) * shaft * UNDERWATER_SHAFT;
}

/// Cheap 2D value noise, for shaping things that are meant to look irregular.
///
/// Hash-based rather than sampled from a texture: it needs no descriptor, and
/// the particle shader is the only caller.
float valueHash(float2 p) {
    return frac(sin(dot(p, float2(127.1, 311.7))) * 43758.5453);
}

float valueNoise(float2 p) {
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    return lerp(lerp(valueHash(i), valueHash(i + float2(1, 0)), u.x),
                lerp(valueHash(i + float2(0, 1)), valueHash(i + float2(1, 1)), u.x), u.y);
}

// ---------------------------------------------------------------------------
// Fog.
//
// **Height-based, and blended toward the sky rather than toward grey.** The
// colour a surface fades into is `skyGradient` evaluated along the view ray,
// which is the same function the background is drawn with — so a distant hill
// dissolves into the exact horizon it stands in front of, at every camera
// angle, with nothing to keep in step. A constant fog colour is the usual
// shortcut and it shows: geometry fades to a grey the sky behind it is not.
//
// The density falls off exponentially with altitude, so fog pools in valleys
// and craters while ridges stand clear. That is what separates fog from a
// distance fade, and on this engine's terrain it is most of the effect.
//
// These are constants for the same reason the sun direction and sky colours
// are: there is no scene-level environment block to author them in. When one
// arrives, all four belong in it together.
// ---------------------------------------------------------------------------

/// Extinction per metre at y = 0.
/// How fast density decays with altitude, per metre. The scale height is its
/// reciprocal — about 33 m here, so a hill twice that stands well clear.

/// Fraction of a surface's colour replaced by fog, for a ray between two
/// points.
///
/// This is the closed-form integral of an exponentially decaying density along
/// the ray, not a per-step march: for density `d * exp(-b*y)` the optical
/// depth has an exact solution, so the whole effect is a handful of ALU with
/// no loop and no samples.
///
/// **`float3`, and the third channel is the point.** Air is achromatic here and
/// every above-water return splats one scalar, so the atmosphere is bit for bit
/// what it was; water is not, and a scalar was the structural reason red could
/// not go first underwater. See the branch below.
float3 fogAmount(float3 from, float3 to) {
    float3 delta = to - from;
    float distance = length(delta);
    if (distance < 1e-4) {
        return float3(0.0, 0.0, 0.0);
    }
    // **Submerged, the same term with the water's own coefficients.** Water's
    // density does not fall off with altitude — the height falloff below is an
    // atmosphere model and would make the sea thinner near the surface — so
    // this is plain Beer-Lambert over the distance. One branch, taken by every
    // surface in the frame at once, which is what makes it an atmosphere
    // rather than a tint on the water shader.
    //
    // **`waterExtinct()`, which is `K_d`, and not the beam coefficient
    // `c = a + b`.** For a ray to a specific point `c` is the textbook operand
    // and it is the wrong one here for two reasons that agree. Physically,
    // ocean scattering is overwhelmingly forward — the light `c` counts as lost
    // is still travelling toward the eye and a wide-field camera collects it,
    // which is why oceanographic *imaging* attenuation sits far nearer `K` than
    // `c`. Structurally, `K_d` is what the surface already attenuates the
    // seabed by, four thousand lines down: `waterBehind` returns
    // `exp(-waterExtinct() * (viewLeg + downLeg))`. Using anything else here
    // would make the same bed, at the same depth, a different colour across the
    // waterline — which is the defect this whole change exists to close. And
    // `c` for Jerlov III is fifteen times this in blue, so the sea would be
    // black at three metres.
    if (eyeUnderwater()) {
        return 1.0 - exp(-distance * waterExtinct());
    }

    float riseRate = delta.y / distance;
    float atOrigin = fogDensity() * exp(-fogFalloff() * from.y);

    float depth;
    // A near-horizontal ray stays at one altitude, and the general form
    // divides by the climb rate — which for that ray is zero. The limit is
    // just constant density times distance.
    if (abs(riseRate) < 1e-4) {
        depth = atOrigin * distance;
    } else {
        float climb = fogFalloff() * riseRate;
        depth = atOrigin * (1.0 - exp(-climb * distance)) / climb;
    }
    // One scalar splatted: air's extinction has no colour in this engine, and
    // splatting it is what keeps every above-water frame identical.
    float above = 1.0 - exp(-depth);
    return float3(above, above, above);
}

/// What fog looks like along a view direction.
///
/// The sun term is why fog reads as weather rather than as a wash: looking
/// into the sun through mist is bright, looking away from it is not, and that
/// asymmetry is most of what makes it convincing.
float3 fogColor(float3 direction) {
    // Underwater there is no sun shaft and no gradient: the medium is the same
    // colour in every direction, and it is the same colour the sky pass paints
    // — so distant geometry dissolves into exactly the background behind it,
    // which is the property the height fog above was written for too.
    if (eyeUnderwater()) {
        return underwaterColor(direction);
    }
    float3 base = skyGradient(direction);
    float toSun = max(dot(direction, sunDirection()), 0.0);
    return base + float3(1.0, 0.93, 0.80) * pow(toSun, 5.0) * 0.30;
}

struct VSOutput {
    float4 position : SV_Position;
    float3 normal : NORMAL;
    float3 color : COLOR;
    // Where this pixel is in the world, which is where its shadow ray starts.
    float3 worldPos : WORLDPOS;
    float2 uv : TEXCOORD0;
    // `nointerpolation`: an index is not a quantity, and interpolating it
    // across a triangle would produce indices no material occupies.
    nointerpolation uint material : MATERIAL;
    // How far a shadow ray must start from this surface to clear it. Per
    // object, because it is set by that mesh's quantisation step.
    nointerpolation float shadowBias : SHADOWBIAS;
    // **Where a ray must start, which is not where the pixel is.** The
    // acceleration structure holds the mesh as it was authored; two things in
    // this shader move the drawn surface away from it — `windBend`, by as much
    // as **1.6 m** on a swaying `forest.loom` tree, and `bodyWave`, by a tenth
    // of a body length. A ray fired from the drawn position starts wherever
    // that displacement happened to put it, which is very often *inside* the
    // rest-pose geometry, and then every visibility query comes back occluded:
    // hard black gashes down the gleamsprat's fringe, and black sides on every
    // tree in the forest.
    //
    // Firing from the undisplaced position makes the query consistent with the
    // thing it queries: a displaced body's shadows, AO and reflections are its
    // rest pose's, which is the bargain ADR 0062 already struck for its
    // reflection and ADR 0064 makes general. Equal to `worldPos` bit for bit on
    // every object that neither sways nor deforms, because it is then the same
    // expression applied to the same vector.
    float3 rayOrigin : RAYORIGIN;
};

// Mirrors `loom_asset::packed::unpack`. Kept in step by a Rust-side test that
// round-trips through the same arithmetic — a decoder that exists only in
// shader source cannot be tested.
float3 unpackPosition(uint packed, float4 unpackParams) {
    float3 q = float3(
        float(packed & 0x3FF),
        float((packed >> 10) & 0x3FF),
        float((packed >> 20) & 0x3FF));
    return unpackParams.xyz + q * unpackParams.w;
}

// Mirrors `loom_asset::packed::unpack`'s UV half. Quantised against the mesh's
// own UV range rather than 0..1, so a wall whose texture tiles eight times
// keeps all 16 bits of precision instead of being clamped flat.
float2 unpackUv(uint packed, float4 unpackParams) {
    float2 q = float2(float(packed & 0xFFFF), float((packed >> 16) & 0xFFFF));
    return unpackParams.xy + q * unpackParams.z;
}

float3 unpackNormal(uint packed) {
    float u = float(packed & 0xFFFF) / 65535.0 * 2.0 - 1.0;
    float v = float((packed >> 16) & 0xFFFF) / 65535.0 * 2.0 - 1.0;
    float z = 1.0 - abs(u) - abs(v);
    float2 xy = float2(u, v);
    if (z < 0.0) {
        // Fold the lower hemisphere back.
        xy = float2(
            (1.0 - abs(v)) * (u >= 0.0 ? 1.0 : -1.0),
            (1.0 - abs(u)) * (v >= 0.0 ? 1.0 : -1.0));
    }
    return normalize(float3(xy, z));
}

/// Main bending: a whole plant leaning away from the wind.
///
/// **The Crytek GPU Gems 3 model, and the step that matters is the last one.**
/// Displacing a vertex horizontally by an amount that grows with height is the
/// easy part; doing only that *stretches* the mesh, because the displaced
/// vertex is now further from the root than it started. At any real wind speed
/// a trunk visibly elongates and a crown tears away from it. Rescaling the
/// offset so the vertex keeps its original distance from the origin is what
/// turns a stretch into a *bend*, and it is the step the paper warns is
/// routinely skipped.
///
/// **Height is metres above the instance's origin, not a normalised
/// coordinate.** The proper model divides by the bounding box height; nothing
/// here knows an instance's bounds, and a per-object bbox is a bigger change
/// than the bend is worth. Metres-with-a-coefficient behaves the same way for
/// anything of ordinary size and is honest about being a coefficient — `sway`
/// is documented in those terms.
///
/// **The phase comes from the instance's world origin**, so neighbouring plants
/// are out of step with each other while every vertex of one plant agrees. The
/// wind field itself is P1's, sampled once at the origin: sampling per vertex
/// would let a crown and a trunk disagree about the weather, and costs a noise
/// evaluation per vertex to do it.
/// **Worked in MODEL space, and the reason is `o.mvp`.** The obvious shape is
/// to bend the world position, but `mvp` already contains the model matrix, so
/// a world-space point cannot be projected with it — and the view-projection
/// alone is not reachable from this entry point. Bending before the model
/// transform keeps one matrix multiply and needs nothing added to any buffer.
///
/// The wind is a world-space field, so the direction is rotated *into* model
/// space: `mul(v, m)` is `transpose(m) * v`, and for a rotation with uniform
/// scale the transpose is the inverse times a constant, which normalising
/// removes. Exact for what a scatter field produces — translation, a yaw and a
/// uniform scale. **Approximate under non-uniform scale**, where the direction
/// is no longer preserved; nothing authors a non-uniformly scaled tree, and
/// this is where to look if something does.
float3 windBend(float3 modelPos, float3x3 m3, float3 origin, float sway) {
    if (sway <= 0.0) {
        return modelPos;
    }
    float height = max(modelPos.y, 0.0);
    if (height <= 1e-4) {
        // The base does not move. A plant that slides across the ground is the
        // "swimming" artifact the grass shader has a long note about.
        return modelPos;
    }

    float3 air = grassWind(origin);
    float2 flow = air.xz;
    float speed = length(flow);
    if (speed <= 1e-4) {
        return modelPos;
    }

    // A gentle sway on top of the steady lean, phased by where the plant is.
    // **Phase from the global world position**, so neighbours are out of step
    // while every vertex of one plant agrees — chunk-local coordinates would
    // put a visible seam wherever voxel chunks meet. `weather.z` is the
    // simulation's clock, never a wall clock (never-do #8).
    float phase = origin.x * 0.7 + origin.z * 0.53;
    float t = push.environment[0].weather.z;
    float gust = 1.0 + 0.25 * sin(t * 1.7 + phase);

    float3 worldFlow = float3(flow.x / speed, 0.0, flow.y / speed);
    float3 localFlow = mul(worldFlow, m3);
    localFlow.y = 0.0;
    float len2 = length(localFlow);
    if (len2 <= 1e-5) {
        return modelPos;
    }
    localFlow /= len2;

    // Quadratic in height: a trunk barely moves at the base and the crown
    // carries the lean, which is how a tree actually loads.
    float amount = sway * gust * height * height * 0.02 * min(speed, 12.0);
    float3 bent = modelPos + localFlow * amount;

    // **Normalise to length, and this is the step the paper warns is skipped.**
    // Displacing a vertex sideways moves it further from the root than it
    // started, so the trunk elongates and the crown tears away from it. Putting
    // the offset vertex back on its original radius turns a stretch into a
    // bend.
    float len = length(bent);
    if (len > 1e-5) {
        bent *= length(modelPos) / len;
    }
    return bent;
}

/// One travelling sinusoid down a body's long axis — ADR 0062.
///
/// `s` is the body coordinate, 0 at the nose and 1 at the tail, from the union
/// bounds of every mesh under the deform node. `inv_len` is **signed**, so the
/// same expression serves a nose at either end of its axis with no branch.
///
/// The envelope is `r * r` over the span past `span_start`: quadratic for the
/// same reason `windBend`'s is, and starting late so a rigid front section
/// stays rigid. On the gleamsprat that is what leaves the chrome pod alone —
/// it ends at 63% of the body and the envelope starts at 58%, so the mirror
/// moves 0.23 mm while the fringe moves 16 mm.
///
/// **`shear` comes back out because the normal has to be corrected in the same
/// breath.** The displacement is a function of exactly one coordinate and
/// points along a different one, so its Jacobian is `I + shear * e_beat (x)
/// e_nose` — rank-1, and its inverse-transpose is `I - shear * e_nose (x)
/// e_beat` exactly. That is three operations on the normal with no tangent
/// attribute, no second texture and no finite difference. On the coral fringe
/// at the authored amplitude the uncorrected normal is wrong by 50 degrees.
///
/// **What that is worth, measured rather than asserted.** Removing the
/// correction and re-rendering `gleamsprat.loom` at tick 104 moves fraction
/// 0.0116, worst channel 92 at 640x400 — real, cheap, and kept. It is *not*
/// what stops the fringe going black: that was the ray origin, and the commit
/// that shipped this term claimed the gashes as its own. See ADR 0064. Do not
/// re-attribute a picture to the term you happened to be writing.
static const float DEFORM_TAU = 6.2831853;

float3 bodyWave(float3 modelPos, ObjectData o, out float shear) {
    shear = 0.0;
    // The same shape `windBend` uses for `sway <= 0`, and it is what keeps
    // every scene authored before `Deform` rendering byte-identically rather
    // than merely within tolerance.
    if (o.deform.x <= 0.0) {
        return modelPos;
    }

    float amplitude = o.deform.x;
    float wavenumber = o.deform.y;
    float frequency = o.deform.z;
    float phase = o.deform.w;

    float lead = o.deformFrame.x;
    float invLen = o.deformFrame.y;
    float spanStart = o.deformFrame.z;
    uint code = uint(o.deformFrame.w + 0.5);
    uint noseAxis = code / 4u;
    uint beatAxis = code % 4u;

    float raw = (lead - modelPos[noseAxis]) * invLen;
    float s = saturate(raw);

    float span = max(1.0 - spanStart, 1.0e-4);
    float rawR = (s - spanStart) / span;
    float r = saturate(rawR);
    float env = r * r;

    // `weather.z` is the simulation's clock, never a wall clock (never-do #8):
    // `--sim N / 60` headless, and a clamped accumulator in the viewer, which
    // is the same deal grass and every tree already have.
    float t = push.environment[0].weather.z;
    float theta = DEFORM_TAU * (s * wavenumber - frequency * t + phase);

    modelPos[beatAxis] += amplitude * env * sin(theta);

    // Zero wherever the body coordinate or the envelope is clamped, because a
    // clamped coordinate has no gradient — this is the flat rest region, and a
    // non-zero shear there would tilt the normals of a part that never moves.
    if (raw > 0.0 && raw < 1.0 && rawR > 0.0) {
        shear = -invLen * amplitude
            * (2.0 * r / span * sin(theta) + env * cos(theta) * DEFORM_TAU * wavenumber);
    }
    return modelPos;
}

[shader("vertex")]
VSOutput vertexMain(uint vertexID: SV_VertexID, uint instanceID: SV_InstanceID) {
    Vertex v = push.vertices[vertexID];
    ObjectData o = push.objects[push.objectOffset + instanceID];

    float3x3 normalMatrix = float3x3(
        o.normalRow0.xyz, o.normalRow1.xyz, o.normalRow2.xyz);

    VSOutput out;
    // `sway` rides in `color.a`, which was a constant 1.0 and unread. The
    // instance's world origin is the model matrix's last column, which is what
    // the wind is sampled at and what phases the sway.
    float3 modelPos = unpackPosition(v.packedPosition, o.unpack);
    // **The vertex as the acceleration structure holds it**, kept before
    // anything displaces it — ADR 0064. Every ray this pixel fires starts here
    // and not where the vertex is drawn, because the TLAS is built from a
    // separate never-touched copy of the mesh and a ray from the drawn position
    // starts wherever the displacement happened to put it: outside the surface,
    // or a metre and a half inside it. See `out.rayOrigin`.
    float3 restPos = modelPos;
    float3 origin = float3(o.model[0][3], o.model[1][3], o.model[2][3]);
    float3x3 m3 = float3x3(o.model[0].xyz, o.model[1].xyz, o.model[2].xyz);
    modelPos = windBend(modelPos, m3, origin, o.color.a);
    // The body wave, and its normal correction in the same breath — ADR 0062.
    // **Corrected before `normalMatrix` is applied**, because the shear is a
    // model-space quantity: the two axis codes name model axes, and applying
    // the world normal matrix first would leave nothing to index.
    float3 n = unpackNormal(v.packedNormal);
    float shear;
    modelPos = bodyWave(modelPos, o, shear);
    uint deformCode = uint(o.deformFrame.w + 0.5);
    n[deformCode / 4u] -= shear * n[deformCode % 4u];
    out.position = mul(o.mvp, float4(modelPos, 1.0));
    out.normal = normalize(mul(normalMatrix, n));
    out.color = o.color.rgb;
    out.material = o.material.x;
    // **Why this is not a constant.** The raster path reconstructs worldPos
    // from a 10-bit quantised position, but the acceleration structure was
    // built from unpacked floats. The two disagree by up to half a
    // quantisation step — on a 16-unit voxel volume that is ~0.008 units,
    // eight times the fixed 1e-3 bias that used to be used here. The ray then
    // starts *inside* the true surface and the volume self-shadows into acne.
    //
    // Scaling by the step this mesh was packed with makes the offset track the
    // error that causes it: a small prop keeps a tight contact shadow, and a
    // large terrain gets the clearance it actually needs.
    //
    // **Known gap, stated rather than hidden.** `unpack.w` is in the mesh's
    // own space, and the offset is applied in world space, so an object scaled
    // by N gets a bias N times smaller than its real error. It is right where
    // the acne actually appears — voxel volumes, which are authored at world
    // scale and drawn unscaled — and too small for a heavily scaled mesh.
    // Correcting it needs the model matrix's scale, which is a change to how
    // this shader indexes a matrix and wants its own commit.
    out.shadowBias = max(SHADOW_BIAS, o.unpack.w);
    // The *bent* position, so lighting, fog and the wetness gate all see the
    // vertex where it actually is. Recomputing the unbent one here would light
    // a tree as though it were standing still.
    out.worldPos = mul(o.model, float4(modelPos, 1.0)).xyz;
    out.rayOrigin = mul(o.model, float4(restPos, 1.0)).xyz;
    out.uv = unpackUv(v.packedUv, o.uvUnpack);
    return out;
}

// The scene, as the ray tracer sees it. Set 0, binding 0 — the renderer's only
// descriptor, because an acceleration structure has no buffer-device-address
// form to reach it by.
[[vk::binding(0, 0)]]
uniform RaytracingAccelerationStructure sceneTLAS;

/// Whether anything blocks `dir` from `origin` within `range`.
///
/// **Ray query, traced inline from this shader.** A shadow ray only asks
/// *whether* anything is in the way, never what or how far, so the traversal
/// can stop at the first hit it finds:
///
/// - `ACCEPT_FIRST_HIT_AND_END_SEARCH` ends traversal at the first blocker
///   instead of continuing to find the nearest one.
/// - `SKIP_PROCEDURAL_PRIMITIVES` is free here — every geometry in the scene
///   is triangles, and saying so lets the hardware skip a branch.
///
/// The flags are template arguments rather than runtime values on purpose:
/// static flags let the driver specialise the traversal, and passing them
/// dynamically forfeits that.
bool anyHit(float3 origin, float3 dir, float tMin, float range) {
    RayDesc ray;
    ray.Origin = origin;
    ray.Direction = dir;
    ray.TMin = tMin;
    ray.TMax = range;

    RayQuery<RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH
           | RAY_FLAG_SKIP_PROCEDURAL_PRIMITIVES> query;
    query.TraceRayInline(sceneTLAS, RAY_FLAG_NONE, 0xFF, ray);
    query.Proceed();
    return query.CommittedStatus() == COMMITTED_TRIANGLE_HIT;
}

/// 1 where the sun reaches this point, 0 where something blocks it. One ray.
///
/// Kept alongside the soft version below because a *secondary* ray does not
/// deserve a soft shadow: this is what a reflected surface is lit by, and
/// spending four rays there to soften a penumbra nobody can resolve in a
/// reflection is the definition of paying twice for nothing.
float sunVisibilityHard(float3 position, float3 normal, float bias) {
    return anyHit(position + normal * bias, sunDirection(), bias, 1e4) ? 0.0 : 1.0;
}

// ---------------------------------------------------------------------------
// Sampling for the secondary rays.
//
// **Deterministic, and that is a hard requirement rather than a nicety.** ADR
// 0010 rejected TAA and ADR 0018 restated it for auto-exposure on the same
// grounds: a frame must be a function of its tick, or the golden gate is
// comparing against an image that no longer exists. So there is no wall clock
// and no frame counter in any of this — every sample direction is a function of
// the pixel's integer coordinate and the sample's index, and re-rendering the
// same tick twice gives the same bits.
//
// The pattern is the R2 low-discrepancy sequence, Cranley-Patterson rotated by
// a per-pixel offset from `loom_hash` — the project's one integer hash, and the
// same one grass places blades with. R2 rather than plain white noise because
// four samples of white noise cluster: the variance of a stratified set of four
// is closer to that of sixteen unstratified, which is the difference between
// four rays being usable and not.
// ---------------------------------------------------------------------------

/// A per-pixel offset in [0,1)², stable for the life of a pixel.
float2 rayDither(float2 pixel) {
    uint x = uint(int(pixel.x));
    uint y = uint(int(pixel.y));
    uint h = loom_hash(loom_hash(x) + y);
    // 24 bits each from two folds of the hash; the low bits of an integer hash
    // are the ones worth trusting least, so both come off the top.
    return float2(float(h >> 8) * (1.0 / 16777216.0),
                  float(loom_hash(h) >> 8) * (1.0 / 16777216.0));
}

/// Sample `i` of the R2 sequence, rotated into this pixel's own offset.
float2 raySample(float2 dither, uint i) {
    // The two-dimensional generalisation of the golden ratio: the reciprocals
    // of the plastic number and its square.
    const float2 R2 = float2(0.7548776662, 0.5698402910);
    return frac(dither + R2 * float(i + 1u));
}

/// An orthonormal pair spanning the plane perpendicular to `axis`.
void rayBasis(float3 axis, out float3 u, out float3 v) {
    // The reference vector must not be parallel to the axis, or the cross
    // product vanishes and the basis collapses to a line — which sends every
    // sample down the same direction and makes a soft shadow hard again.
    float3 reference = abs(axis.y) < 0.99 ? float3(0.0, 1.0, 0.0) : float3(1.0, 0.0, 0.0);
    u = normalize(cross(reference, axis));
    v = cross(axis, u);
}

// ---------------------------------------------------------------------------
// Soft sun shadows.
//
// The sun is a disc, not a point, so a shadow edge has a penumbra whose width
// grows with the distance from blocker to receiver. One ray per pixel cannot
// express that at all; `SUN_RAYS` of them, spread over the disc, is the whole
// mechanism.
//
// **Wider than the real sun, deliberately, and the measurement is what says
// so — and it is an angular *radius*, not a diameter.** The sun's disc is
// 0.53° across as seen from Earth, so its radius is 0.265° — and the smallest
// row measured below is 0.5°, already nearly twice the real sun. Even there the
// penumbra is at the noise floor: a 1-ray render of `materials` differs from a
// 64-ray one in 0.13% of pixels, against the golden gate's own 0.1% threshold
// for "a different image". Four rays at anything physical would be three rays
// spent on something nobody can see.
//
// So the angle is a *look* parameter, not a physical one, and what is measured
// is how far past physical it has to go before the penumbra exists. Converged
// (64-ray) renders of `materials` at 640x400, each against the 0.5° one:
//
//     angle    mean     worst   pixels past tolerance
//     2.0deg   0.0139   44      0.28%
//     4.0deg   0.0404   58      0.58%
//
// **2° is the smallest widening the project's own gate would call a different
// picture** — 0.28% against its 0.1% — and it gives a penumbra of about 3.5 cm
// per metre of blocker distance, which is narrow enough that a contact shadow
// stays a contact shadow. 4° doubles the noise for an edge that reads as soft
// rather than as a shadow.
//
// A fuller model exists and was not built: the effective source is the whole
// bright part of the sky, so it should widen with `cloudCover()`, which this
// file already couples the sun's strength and colour to. That is one `lerp`,
// and it wants a scene authored under clear sky next to one under overcast to
// judge it against.
static const float SUN_ANGLE_DEGREES = 2.0;
/// The disc's radius one unit along the ray, which is what the sample offset
/// is measured in.
static const float SUN_SPREAD = tan(radians(SUN_ANGLE_DEGREES));

/// **Four, chosen where the cost curve breaks, not where the noise does.**
///
/// Noise against a 64-ray reference at 2°, on `materials` at 640x400, and cost
/// on `lanternhead` at 1920x1080 with AO stubbed out, forward pass, median of
/// three (0.447 ms for the single hard ray this replaces):
///
///     rays          1       2       4       8       16
///     worst         77      43      36      28      26
///     mean          .0257   .0160   .0115   .0071   .0047
///     forward ms    0.452   0.456   0.467   0.559   0.851
///
/// Rays two to four cost 0.005 ms *each* — a sun ray is coherent, every pixel
/// sending one in nearly the same direction, and the hardware hides three of
/// them behind one. Rays five to eight cost 0.023 ms each and rays nine to
/// sixteen 0.037 ms each, while the worst-channel error has already fallen from
/// 77 (above the gate's 72) to 36 (well under it) by ray four.
///
/// So the cheap half of the curve ends exactly where the noise stops mattering.
static const uint SUN_RAYS = 4;

/// 1 where the sun reaches this point, 0 where something blocks it, and
/// everything between in a penumbra.
float sunVisibility(float3 position, float3 normal, float bias, float2 pixel) {
    // Offset along the normal, not just by TMin: a ray starting exactly on the
    // surface hits the surface it came from, and the whole scene self-shadows
    // into acne.
    float3 origin = position + normal * bias;
    float3 toSun = sunDirection();
    float3 u, v;
    rayBasis(toSun, u, v);

    float2 dither = rayDither(pixel);
    float open = 0.0;
    for (uint i = 0; i < SUN_RAYS; ++i) {
        float2 xi = raySample(dither, i);
        // Uniform over the disc: the sqrt is what stops the samples piling up
        // in the middle, which would narrow the penumbra back toward a hard
        // edge for no saving.
        float radius = SUN_SPREAD * sqrt(xi.x);
        float angle = 6.2831853 * xi.y;
        float3 dir = normalize(toSun + u * (radius * cos(angle))
                                     + v * (radius * sin(angle)));
        open += anyHit(origin, dir, bias, 1e4) ? 0.0 : 1.0;
    }
    return open / float(SUN_RAYS);
}

// ---------------------------------------------------------------------------
// Ray-traced ambient occlusion.
//
// **This is the answer to the SSAO deferral**, and it arrives by sidestepping
// what blocked it rather than by solving it. `docs/design/POST-STACK-PLAN.md`
// deferred screen-space AO because a grass field puts 45,000 blades into the
// depth buffer, and a hemisphere kernel reading that depth hairball sees one
// enormous concavity and darkens the whole meadow. The objection is entirely
// about the *depth buffer*, and a ray does not read one: grass is generated in
// a vertex shader from `SV_VertexID` with no vertex or index buffer, so it is
// not in the acceleration structure and cannot be hit. The blades therefore
// occlude nothing, which is exactly the behaviour SSAO could not be made to
// have.
//
// It is worth being plain that this is a *coincidence of the implementation*
// and not a physical claim — real grass does occlude — but it is the behaviour
// the project wants, and it is stable rather than tuned.
//
// Cosine-weighted, so the estimate is the ambient visibility term directly:
// no division by a normalisation constant, and no cosine factor to apply after.
// ---------------------------------------------------------------------------

/// How far a ray looks for an occluder, in metres.
///
/// **A range, not infinity, and the two halves of the measurement disagree.**
///
/// AO is contact darkening — the corner where two walls meet, the ground under
/// a crate. Against a stubbed build on `primitives` at 640x400 with 32 rays, so
/// the numbers are the range's doing and not the sampling's:
///
///     range      0.25    0.5     1.0     2.0     4.0     inf
///     worst      32      70      80      84      84      86
///     mean       .0056   .0536   .1506   .2796   .3839   .4023
///
/// `worst` is how dark the deepest contact gets and `mean` is how much the
/// whole frame dims. **They saturate at very different rates, and that gap is
/// the entire signal.** By 1 m the deepest contact is already at 93% of its
/// unbounded value while the overall dimming is at 37% of it. Going on to 2 m
/// buys 5% more contact for double the flat dimming; stopping at 0.5 m gives
/// up 12% of the contact for very little.
///
/// So 1 m is where contact darkening is nearly all bought and global dimming is
/// still mostly unpaid for. It is also about a human stride, which is the scale
/// these scenes are authored at, and small enough that the term stays a
/// *lighting* correction rather than a crude one-bounce GI.
static const float AO_RANGE = 1.0;

/// **Four per lane, and the quad makes it sixteen.**
///
/// Was eight per pixel, unshared. Noise against a 64-ray render of the same
/// build at 960x600 — worst channel, mean, and salt *added* at threshold 8,
/// which is the number that can see grain — beside the whole forward pass at
/// 1920x1080, min of three fresh processes x the last sixteen of twenty-four
/// frames, GPU warmed, quiet box at load 1.2-1.7:
///
///     scene         8 unshared (was)    4 + share (now)    forward ms
///     primitives    48 / .0492 / 2408   29 / .0347 /  440   0.238 -> 0.181
///     cave          17 / .0143 /  430   22 / .0118 /   59   0.533 -> 0.390
///     materials     29 / .0087 /   28   14 / .0065 /   -5   0.372 -> 0.280
///     stoneyard     28 / .0621 /  150   68 / .0562 /  163   0.985 -> 0.724
///     croft         22 / .0411 /   91   40 / .0357 /   61   2.207 -> 1.691
///     forest        25 / .0831 /  139   25 / .0599 /   10   1.177 -> 0.832
///     spruce        40 / .0136 /   56   40 / .0120 /   30   0.983 -> 0.698
///     lanternhead   12 / .0014 /    7   12 / .0011 /    9   0.868 -> 0.655
///
/// **23-29% off the forward pass for a picture measurably closer to the truth**
/// — mean error falls on 11 of the 13 scenes swept, and `primitives`' grain
/// falls 2408 -> 440. The two it does not are `proving_ground` (.0055 ->
/// .0090) and `gleamsprat` (.0058 -> .0062), both scenes whose absolute AO is
/// a fifth of `croft`'s at any setting.
///
/// **The share is what buys this, not the ray count.** Four rays with no share
/// is the control, and it is worse than eight on every scene: `primitives`
/// 67 / .0834 / 5989, `cave` 24 / .0236 / 2280, `forest` 29 / .1449 / 678.
/// Halving the rays alone is a straight loss; halving them and decorrelating
/// the four lanes is a gain.
///
/// **The worst-channel rises are single pixels, and `worst` cannot say so.**
/// `stoneyard`'s 68 is **one pixel of 576,000** (at (100,315); the next is 32),
/// `croft`'s 40 is one, `proving_ground`'s 36 is two. Salt at threshold 24 —
/// which is what sees a speck — goes *down* on `stoneyard`, to -5.
///
/// **The occupancy cliff this constant used to be justified by does not
/// reproduce.** The old note here recorded 0.024 ms per ray to eight and 0.101
/// ms each from eight to sixteen. Measured again on the same scene, unshared,
/// forward pass at 1920x1080, marginal ms per added ray between rungs:
///
///     rays          2      4      8     16     |  per-ray 2->4  4->8  8->16
///     lanternhead  0.577  0.666  0.868  1.298  |    .0445  .0505  .0538
///     stoneyard    0.607  0.730  0.985  1.502  |    .0615  .0638  .0646
///     cave         0.324  0.395  0.535  0.807  |    .0355  .0350  .0340
///     croft        1.458  1.701  2.207  3.246  |    .1215  .1265  .1299
///     materials    0.238  0.283  0.375  0.561  |    .0225  .0230  .0233
///
/// Flat, on five scenes, across the rung the cliff was claimed at. The loop is
/// rolled, so the trip count cannot move the register footprint. **Anything
/// that cites that cliff should be re-derived or struck** — the water
/// reflection's one-ray choice below is one such citation, and its real reason
/// is the salt measurement beside it, not the cliff.
///
/// An AO ray is still far more expensive than a sun ray at the same count, and
/// the reason is coherence: every pixel's sun rays go nearly the same way and
/// share their traversal, while a hemisphere sample goes wherever the surface
/// faces. That is why this term, and not the sun's, is the one worth halving.
/// **The dial, and it is a run-time one.** Rays per lane; the quad sees four
/// times this because the four lanes take disjoint slices of one sequence.
/// `LOOM_AO_RAYS` sets it, the default is 4, and `loom_render`'s crate docs
/// carry the table of what each stop costs in milliseconds and in pixels — one
/// place, in English, for someone who is choosing rather than reading this.
///
/// **A buffer read rather than a `static const`.** The loop was already rolled
/// with a run-time trip count (`quadShare` picks between two values and is a
/// per-material fact), so nothing here was ever unrolled and nothing is lost by
/// letting the count vary: measured at 4, the compiled shader renders the same
/// pixels and the same milliseconds as the constant did. What it buys is that
/// the human can trade grain for frame time on their own machine without a
/// `slangc` run — and the honest reason to want that is that the cheap stop and
/// the expensive stop are both *defensible*, which is unusual, so the choice
/// belongs to whoever is looking at the screen.
///
/// `viewport.zw` was two free trailing scalars in the environment buffer, so
/// this moved no offset and re-pinned no layout test. Stamped by `renderer.rs`
/// and `viewer.rs` beside the width and height, which is the same "the renderer
/// knows this and the scene does not" rule those two already follow. `max` so a
/// caller that leaves it zero gets a picture rather than a divide by zero.
uint aoRays() {
    return max(1u, uint(push.environment[0].viewport.z));
}

/// What a lane fires when it cannot share — see `cutoutMaterial` in
/// `fragmentMain`. **Twice the shared count**, which at the default is the
/// eight rays this term had before the share existed: a lane with no
/// neighbours to borrow from is the old shader, and it should cost and look
/// like the old shader at every stop of the dial.
uint aoRaysUnshared() {
    return aoRays() * 2u;
}

/// The fraction of the hemisphere above `normal` that is open, within
/// [`AO_RANGE`].
///
/// `quadShare` must be **quad-uniform**. It reads the other three lanes of the
/// 2x2, so a per-pixel value here reads lanes that took the other branch, and
/// that is undefined.
float ambientVisibility(float3 position, float3 normal, float bias, float2 pixel,
                        bool quadShare) {
    float3 origin = position + normal * bias;
    float3 u, v;
    rayBasis(normal, u, v);

    // **The dither is the QUAD's, and the lane slices the sequence.** Seeding
    // from the quad's own corner and indexing at `lane * rays + i` is what
    // makes the four lanes' samples one stratified set of `4 * aoRays()`
    // directions rather than four rotated copies of `aoRays()` — four rotations
    // of one set are four correlated estimates, and averaging them buys much
    // less than averaging four disjoint slices does. It costs three integer
    // ops and it is most of why halving the ray count is not a quality loss.
    //
    // Still a pure function of the integer pixel coordinate, so the dither is
    // still screen-locked: it sits still while the world moves under it. It is
    // locked to a 2x2 grid now rather than to a single pixel, which is the only
    // thing about ADR 0019's determinism argument that moves.
    //
    // Offset by `(37, 11)` for the reason it always was: two consumers of
    // `rayDither` sharing one pattern correlate, and a pixel's AO rays must not
    // be the same directions its shadow rays used.
    uint px = uint(int(pixel.x));
    uint py = uint(int(pixel.y));
    uint lane = quadShare ? ((px & 1u) | ((py & 1u) << 1u)) : 0u;
    float2 seed = quadShare ? float2(float(px & ~1u), float(py & ~1u)) : pixel;
    uint rays = quadShare ? aoRays() : aoRaysUnshared();
    float2 dither = rayDither(seed + float2(37.0, 11.0));
    float open = 0.0;
    for (uint i = 0; i < rays; ++i) {
        float2 xi = raySample(dither, lane * rays + i);
        // Cosine-weighted: a disc sample lifted onto the hemisphere. Uniform
        // sampling would need the cosine applied afterwards and would spend
        // most of its rays near the horizon, where the weight is nearly zero.
        float radius = sqrt(xi.x);
        float angle = 6.2831853 * xi.y;
        float3 dir = u * (radius * cos(angle))
                   + v * (radius * sin(angle))
                   + normal * sqrt(max(1.0 - xi.x, 0.0));
        open += anyHit(origin, normalize(dir), bias, AO_RANGE) ? 0.0 : 1.0;
    }
    float mine = open / float(rays);
    if (!quadShare) {
        return mine;
    }
    // **The whole reconstruction filter, and its guide is a guarantee rather
    // than a heuristic.** A fragment quad is generated from one primitive, so
    // these are four points on one surface — there is nothing for a depth or
    // normal weight to reject, and the bright halo a depth-guided upsample
    // produces cannot occur because this filter never crosses a silhouette.
    // Anything wider than a quad is refused on determinism rather than
    // capability: the screen-space arrangement of the quads inside a subgroup
    // is implementation-defined, and a 2x2 is fixed by the pixel grid.
    return (mine + QuadReadAcrossX(mine) + QuadReadAcrossY(mine)
                 + QuadReadAcrossDiagonal(mine)) * 0.25;
}

// ---------------------------------------------------------------------------
// Ray-traced reflections.
//
// **What is in the acceleration structure, and what is not.** The TLAS holds
// one instance per `Object` — meshes, and nothing else. Grass, water, rain,
// fire and smoke are all generated in a vertex shader from `SV_VertexID` with
// no vertex or index buffer, so none of them is in it and none of them can be
// hit by a ray. A reflection is therefore a reflection of *the meshes*, and
// this is stated here rather than discovered later because the scene the
// feature was built against is exactly the one where it shows: in
// `lanternhead` a wet quay at `WET_SMOOTH` roughness now carries the shed, the
// brazier's drum and the lantern posts, and does not carry the flame two
// metres from it. See ADR 0019 for what was measured and why it ships anyway;
// the short version is that the fire's *light* does arrive, because the hit is
// shaded by the same `pointLights` the primary surface is, and a brazier's
// contribution to a reflection is mostly its pool of orange light rather than
// its flame.
//
// The miss path is the term this replaces: the analytic sky gradient. So a
// scene with nothing to reflect renders as it did, and the change is additive
// rather than a swap.
// ---------------------------------------------------------------------------

/// How far a reflection ray looks, in metres.
///
/// **A bound, because fog makes one free.** Past a certain distance a hit is
/// already fully hazed and contributes nothing but a full traversal. Measured
/// on `lanternhead` at 960x600 against a 4 km build, worst channel difference:
///
///     range    15 m   30 m   60 m   120 m   240 m
///     worst    24     1      0      0       0
///
/// 60 m is *bit for bit* the unbounded answer on this scene and on `materials`,
/// which is the smallest value that can be said of. 30 m is one channel off in
/// one pixel; 15 m is visibly short.
///
/// **This is per-scene in a way the number does not admit.** It is set by how
/// thick the fog is, and a scene authored clear would want more. The honest
/// version reads `fogDensity()` and solves for where the transmittance falls
/// below a threshold — worth doing when a clear-air scene with a mirror in it
/// exists, and not before.
static const float REFLECT_RANGE = 60.0;

/// Ceiling on the radiance one reflection ray may report.
///
/// **This is the firefly clamp, and the feature does not ship without it.** A
/// reflected hit is shaded by the same `pointLights` the primary surface is,
/// and that term is a windowed inverse square: neighbouring pixels on a rippled
/// wet surface send their rays to quite different places, and a ray that lands
/// close to a lantern comes back with a radiance two orders of magnitude above
/// its neighbours. One sample of a high-variance integrand is what a firefly
/// *is*. `lanternhead` showed it immediately — isolated orange pixels scattered
/// across the quay, worst-channel error 173 against a stubbed build, well past
/// the golden gate's own 72.
///
/// The two standard cures are both closed to this project: more samples per
/// pixel (measured below, and the cost is not in the budget) and temporal
/// accumulation (ADR 0010, ADR 0018 — a frame must stay a function of its
/// tick). Clamping is the third, it is one `min`, and the codebase already
/// does exactly this at `LIGHT_MAX` for the singularity of a single light.
///
/// Chosen by counting salt — pixels more than 24/255 from the median of their
/// eight neighbours — on `lanternhead` at 960x600. The stubbed build already
/// scores 8,174 from the ripples and the rain, so what matters is what the
/// reflection *adds*:
///
///     clamp        1      2      4      8      16     64     none
///     salt added   0      3      10     37     39     39     39
///     worst        36     36     36     56     86     166    173
///     mean         .0954  .0970  .0973  .0979  .0990  .1010  .1006
///
/// **4.0.** Below it the term is still gaining brightness; above it the salt
/// nearly quadruples and the worst-channel error leaves the floor of 36 that
/// three different clamps agree on. At 4.0 the reflection adds 10 salt pixels
/// in 576,000 and keeps 97% of the brightness the unclamped version has.
///
/// The unclamped worst of 173 is the number that matters most here: it is more
/// than twice the golden gate's own 72, so an unclamped build would have been
/// failing the gate on noise rather than on content.
static const float REFLECT_MAX_RADIANCE = 4.0;

/// What the surface at `position` sees along `dir`.
///
/// `dir` is the roughness-widened lobe direction, not the mirror direction:
/// bending toward the normal as roughness rises is the same cheap stand-in for
/// a prefiltered environment the sky-only version used, and keeping it means a
/// slightly rough surface reflects a slightly blurred scene rather than a sharp
/// one.
///
/// **There is no roughness cutoff, and one was measured and removed.** The
/// obvious shape for this is "trace only below some roughness, sky above it",
/// and a single mirror ray genuinely is a poor sample of a wide lobe. But the
/// blend at the bottom of this function already weights the traced answer to
/// zero as roughness reaches 1, so a cutoff is a *cost* argument, and the cost
/// is not there. `lanternhead` at 1920x1080, forward pass, against a build with
/// the trace stubbed out at 0.782 ms:
///
///     cutoff       0.10    0.20    0.35    0.50    1.00 (none)
///     forward ms   0.776   0.801   0.839   0.857   0.885
///     pixels       0%      0.002%  1.51%   3.00%   4.39%
///     salt added   0       0       3       -4      20
///
/// Picture-per-millisecond is flat across every band — 2.5, 3.3 and 3.5 units
/// of mean difference per millisecond going up — so there is no knee to put a
/// cutoff at, and the salt count says the wide-lobe samples are not producing
/// blotches on these scenes. Removing it deletes a constant and a branch. If a
/// scene ever does show single-sample blotching on a rough surface, the fix is
/// the cutoff, and this table is where it goes back.
/// **`miss` is what the ray sees when it hits nothing, and it is a parameter
/// rather than `skyGradient(dir)` because water needs a different sky.** The
/// opaque path reflects `skyGradient` — a gradient with no sun disc and no
/// cloud — which is the right stand-in for a prefiltered probe on a rough
/// metal. `waterFragmentMain` has always reflected `skyColor`, and the sun's
/// glitter path on a sea IS the reflected sun disc; passing the gradient there
/// would delete it. Same function, two skies, one line.
float3 tracedEnvironment(float3 position, float3 normal, float3 dir,
                         float roughness, float bias, float3 miss) {
    float3 sky = miss;
    float3 origin = position + normal * bias;
    RayDesc ray;
    ray.Origin = origin;
    ray.Direction = dir;
    ray.TMin = bias;
    ray.TMax = REFLECT_RANGE;

    // **No `ACCEPT_FIRST_HIT_AND_END_SEARCH` here**, unlike every other ray in
    // this file. A shadow ray wants any blocker; a reflection wants the
    // *nearest* surface, and the first one traversal happens to find is not it.
    // Using the shadow flags here compiles, runs, and reflects whatever the
    // BVH visited first — which looks like the right objects in the wrong
    // order and is very hard to read as a bug.
    RayQuery<RAY_FLAG_SKIP_PROCEDURAL_PRIMITIVES> query;
    query.TraceRayInline(sceneTLAS, RAY_FLAG_NONE, 0xFF, ray);
    query.Proceed();
    if (query.CommittedStatus() != COMMITTED_TRIANGLE_HIT) {
        return sky;
    }

    // **`CommittedInstanceID` is the object index**, packed into the TLAS
    // instance's custom index by `raytrace.rs::build_instances` from the same
    // mesh-sorted slice `pack_objects` fills this buffer from. That is the
    // whole channel from traversal back to the shader, and it is enough for a
    // colour without a second buffer or a descriptor.
    ObjectData hitObject = push.objects[query.CommittedInstanceID()];
    // `hitT`, not `distance` — Slang has a builtin of that name and shadowing
    // it inside a function that also fogs over a distance is a trap.
    float hitT = query.CommittedRayT();
    float3 hit = origin + dir * hitT;

    // **The hit triangle, which is what makes this a reflection of a surface
    // rather than of a silhouette.** ADR 0021 shaded with the material's mean
    // albedo because "fetching a texel would need the triangle's UVs, which
    // needs the index buffer, which is one more pointer than the push block
    // has room for". The premise was right and the conclusion was not: the
    // pointer does not have to be in the push block. `ObjectData.indices`
    // carries it per object, in the two `uint` lanes `material` was padding
    // with, and the record did not grow.
    //
    // `material.y` is the mesh's first index; `CommittedPrimitiveIndex` counts
    // from the start of that mesh's BLAS build range, whose `primitiveOffset`
    // is `first_index * 4` bytes.
    uint slot = hitObject.material.y + query.CommittedPrimitiveIndex() * 3u;
    Vertex hv0 = push.vertices[hitObject.indices[slot]];
    Vertex hv1 = push.vertices[hitObject.indices[slot + 1u]];
    Vertex hv2 = push.vertices[hitObject.indices[slot + 2u]];
    float2 bary = query.CommittedTriangleBarycentrics();
    float3 bw = float3(1.0 - bary.x - bary.y, bary.x, bary.y);

    // **The interpolated vertex normal, not the triangle's cross product.**
    // Both are now reachable; this one is what the rasteriser uses, so a
    // sphere reflects as a sphere instead of as a facetted one, and a
    // reflection agrees with the direct view of the same surface. It replaces
    // `-dir`, which was exact only where the surface faced the ray head-on and
    // wrong by the angle of incidence everywhere else.
    float3 objectNormal = bw.x * unpackNormal(hv0.packedNormal)
                        + bw.y * unpackNormal(hv1.packedNormal)
                        + bw.z * unpackNormal(hv2.packedNormal);
    float3 hitNormal = normalize(float3(dot(hitObject.normalRow0.xyz, objectNormal),
                                        dot(hitObject.normalRow1.xyz, objectNormal),
                                        dot(hitObject.normalRow2.xyz, objectNormal)));
    // A ray hits either side of a triangle and nothing here is single-sided.
    // Without this a reflection of a box's far wall lights from behind.
    if (dot(hitNormal, dir) > 0.0) {
        hitNormal = -hitNormal;
    }

    float2 hitUv = bw.x * unpackUv(hv0.packedUv, hitObject.uvUnpack)
                 + bw.y * unpackUv(hv1.packedUv, hitObject.uvUnpack)
                 + bw.z * unpackUv(hv2.packedUv, hitObject.uvUnpack);

#ifdef REFLECT_INDEX_DEBUG
    // **The test that says the indexing is right, rather than that it looks
    // right.** Everything above is arithmetic on a primitive index whose
    // origin is a convention — `first_index` plus three times a number
    // traversal reports — and a wrong convention reads a *different real
    // triangle*, which produces a plausible texture and no error anywhere.
    //
    // The three vertices this decoded also carry positions. Interpolating them
    // by the same barycentrics must reproduce the point the ray query itself
    // reports as the hit, to within packing error. Amplified 80x, this returns
    // black if and only if the triangle is the one that was hit.
    //
    // To run it: put `#define REFLECT_INDEX_DEBUG` at the top of this file,
    // render `stoneyard`, and look at the sphere. It is not a gate, because it
    // costs three more decodes per reflected pixel to prove something that is
    // either true for every triangle or false for every triangle.
    //
    // Run on `stoneyard` and `materials` at 1600x1000: **black**, and the
    // control — `+ 1u` on `slot`, i.e. deliberately off by one vertex — is
    // vivid green and red over the whole reflection.
    float3 reconstructed = mul(hitObject.model, float4(
        bw.x * unpackPosition(hv0.packedPosition, hitObject.unpack)
      + bw.y * unpackPosition(hv1.packedPosition, hitObject.unpack)
      + bw.z * unpackPosition(hv2.packedPosition, hitObject.unpack), 1.0)).xyz;
    return abs(reconstructed - hit) * 80.0;
#endif

    float3 albedo = hitObject.color.rgb;
    if (hitObject.material.x != NO_TEXTURE) {
        MaterialData hm = push.materials[hitObject.material.x];
        float lod = reflectLod(hitT, roughness);
        albedo = reflectedAlbedo(hm, hit, hitNormal, hitUv, lod, true);

        // **The steep-slope layer, and leaving it out is wrong rather than
        // merely incomplete.** `lanternhead`, `ground`, `beach` and `meadow`
        // all author a base material plus a layer blended in by slope; on
        // `lanternhead` the flats are stone and the banks are soil, so a
        // base-only fetch reflects a bank as stone with full conviction. The
        // blend needs a world position and a real geometric normal, and this
        // is the first version of this function to hold either.
        if (hm.maps.w != NO_TEXTURE) {
            MaterialData lm = push.materials[hm.maps.w];
            albedo = lerp(albedo,
                          reflectedAlbedo(lm, hit, hitNormal, hitUv, lod, false),
                          groundLayerWeight(hit, hitNormal, lm.params.x));
        }
    }

    float visibility = sunVisibilityHard(hit, hitNormal, bias);
    float3 shade = albedo
        * (hemisphereAmbient(hitNormal) * ambientStrength()
           + pointLights(hit, hitNormal)
           + sunStrength() * max(dot(hitNormal, sunDirection()), 0.0) * visibility);
    // See `REFLECT_MAX_RADIANCE`. Per channel rather than on the luminance, so
    // a clamped firefly loses brightness and not its hue.
    shade = min(shade, REFLECT_MAX_RADIANCE);

    // Fogged over the distance the ray travelled, so a reflection recedes into
    // the same haze the direct view of that surface would. Without it a wet
    // quay reflects a crisp shed standing in front of a fogged one.
    shade = lerp(shade, fogColor(dir), fogAmount(origin, hit));

    // Toward the sky as the lobe widens, reaching it exactly at roughness 1 —
    // which is the old sky-only term, bit for bit, for a fully rough surface.
    return lerp(shade, sky, saturate(roughness));
}

// ---------------------------------------------------------------------------
// Materials.
//
// Every texture in the scene lives in one array, bound once for the pass. A
// material names its maps by index, so a scene with fifty materials still
// draws in one instanced call — which is the whole point of never-do #2.
// ---------------------------------------------------------------------------

[[vk::binding(0, 1)]]
uniform Sampler2D textures[];

/// Sample a texture from the bindless array.
///
/// `NonUniformResourceIndex` is required, not decorative: instances inside one
/// draw call can carry different materials, so the index varies across a wave.
/// Without it the hardware is permitted to sample whichever index the first
/// lane happened to hold, and the symptom is one object wearing another's
/// texture — intermittently, depending on how draws pack into waves.
float4 sampleMap(uint index, float2 uv) {
    return textures[NonUniformResourceIndex(index)].Sample(uv);
}

/// As [`sampleMap`], at a mip level the caller chose.
///
/// **`SampleLevel` is mandatory for anything a ray hit, not a preference.**
/// Implicit-derivative sampling asks the neighbouring lanes in the quad what
/// UV *they* got; for a reflection those lanes may have hit a different
/// object, a different mesh, or the sky, so the derivative is meaningless and
/// the chosen mip is whatever that noise implies. It compiles, it looks nearly
/// right in a still, and it twinkles the moment the camera moves — the exact
/// failure class a golden PNG cannot see.
float4 sampleMapLevel(uint index, float2 uv, float lod) {
    return textures[NonUniformResourceIndex(index)].SampleLevel(uv, lod);
}

/// How coarse a mip a reflected hit reads.
///
/// Two terms, because two things blur a reflection. **Distance**: the ray's
/// footprint grows with how far it travelled, exactly as a mip chain assumes,
/// and `log2(1 + t)` is that growth in mip units with a sane value at t = 0.
/// **Roughness**: the direction handed to this function is already the
/// roughness-widened lobe, so the surface it lands on should arrive blurred by
/// the same amount, and blurring it with mips is far cheaper than more rays.
///
/// `ponytail:` the base is a constant, not a footprint. The honest version
/// projects a cone of the lobe's angle onto the hit surface and converts its
/// area to a mip level, which needs the texture's dimensions — reachable
/// through `GetDimensions`, at the cost of a query per reflected pixel. Worth
/// doing if a reflection ever reads too soft on a close, near-mirror surface.
///
/// **The roughness coefficient was measured, against the right reference.**
/// High-frequency energy (mean |Laplacian| of luminance) in the reflected band
/// at 3200x2000, against the *direct* view of the same surface in the same
/// frame:
///
///     coefficient        4.0     2.0     direct view
///     materials floor    4.73    5.11    5.00
///     stoneyard ground  10.43   11.32   18.61
///
/// Halving it buys 8–9% and takes `materials` *past* its own direct view — and
/// a reflection compresses a surface into fewer pixels, so more high-frequency
/// energy per pixel than the thing itself is undersampling. The sharper-looking
/// number is the one that aliases; `xtask shimmer` holds the camera still and
/// cannot see the difference, so this is the reference that decides it.
static const float REFLECT_LOD_BASE = 0.5;
static const float REFLECT_LOD_ROUGHNESS = 4.0;

float reflectLod(float t, float roughness) {
    return REFLECT_LOD_BASE + log2(1.0 + t) + roughness * REFLECT_LOD_ROUGHNESS;
}

/// How much coarser the macro variation is than the detail tiling.
///
/// A tiled texture reads as a repeating stamp the moment you can see more than
/// a few tiles at once, and terrain is nothing but that. Sampling the *same*
/// texture at a small fraction of the frequency gives broad lush and dry
/// patches that break the repeat without needing a second texture, a second
/// UV set, or anything authored.
static const float MACRO_SCALE = 1.0 / 26.0;
/// Luminance the modulation is neutral at, so the macro term redistributes
/// brightness instead of darkening everything.
static const float MACRO_PIVOT = 0.36;
static const float MACRO_STRENGTH = 1.15;

/// Project a texture down the three world axes and blend by the normal.
///
/// **This is what voxel terrain needs.** Surface Nets places a vertex anywhere
/// inside its cell and produces no UVs at all, so there is nothing to unwrap.
/// Three samples instead of one is the honest cost, which is why it is a
/// per-material opt-in rather than the default.
float4 triplanar(uint index, float3 position, float3 normal) {
    // Raised to a power so the blend is tight: a linear weight smears all
    // three projections across most of a curved surface, which reads as a
    // blurred texture rather than a projected one.
    float3 weight = pow(abs(normal), 4.0);
    weight /= max(weight.x + weight.y + weight.z, 1e-4);
    // **`.zy`, not `.yz`.** The X plane's two axes must arrive in the order
    // `triplanarNormal`'s whiteout blend and final swizzle assume, and albedo
    // must agree with relief or the grain lights against its own shape.
    return sampleMap(index, position.zy) * weight.x
         + sampleMap(index, position.xz) * weight.y
         + sampleMap(index, position.xy) * weight.z;
}

/// As [`triplanar`], at an explicit mip. See [`sampleMapLevel`] for why a ray
/// hit cannot use the implicit one.
float4 triplanarLevel(uint index, float3 position, float3 normal, float lod) {
    float3 weight = pow(abs(normal), 4.0);
    weight /= max(weight.x + weight.y + weight.z, 1e-4);
    return sampleMapLevel(index, position.zy, lod) * weight.x
         + sampleMapLevel(index, position.xz, lod) * weight.y
         + sampleMapLevel(index, position.xy, lod) * weight.z;
}

/// The albedo a reflected hit shades with.
///
/// The raster path's texture logic minus everything a reflection cannot use:
/// no alpha cutout (`raytrace.rs` keeps cutout objects out of the TLAS, so
/// there is nothing here to cut out) and no normal map.
///
/// Falls back to `meanAlbedo` when the material has no map, which keeps ADR
/// 0021's guarantee exactly: with no texture the mean *is* the authored albedo,
/// so an untextured surface reflects bit for bit as it did.
///
/// **`macro` is not a toggle, it is which surface this is.** The raster path
/// applies macro variation to the base material and *not* to the steep-slope
/// layer; a reflection that applied it to both would disagree with the direct
/// view of the same bank, which is the whole point of reading real geometry.
float3 reflectedAlbedo(MaterialData m, float3 hit, float3 normal, float2 uv, float lod, bool macro) {
    if (m.maps.x == NO_TEXTURE) {
        return m.meanAlbedo.rgb;
    }
    // Scaling the coordinate rather than the sample, as the raster path does:
    // for triplanar this is world position, so a shared texture keeps one
    // real-world size across every object however each one is scaled.
    float3 projected = hit * float3(m.params.z, m.params.w, m.params.z);
    if ((m.maps.z & FLAG_TRIPLANAR) == 0) {
        // **A real UV, which is what this whole change bought.** Feeding
        // triplanar a `packedUv` instead would sample a real texture at
        // plausible-looking wrong coordinates — worse than a flat colour,
        // because it is harder to disbelieve.
        return m.albedo.rgb * sampleMapLevel(m.maps.x, uv * m.params.zw, lod).rgb;
    }
    float3 albedo = m.albedo.rgb * triplanarLevel(m.maps.x, projected, normal, lod).rgb;
    if (macro) {
        // One extra sample, top-down only, exactly as the raster path takes
        // it. Without it a reflected terrain is uniformly bright where the
        // surface it reflects has broad lush and dry patches, and the two do
        // not agree where they meet — measured on `stoneyard`: reflected mean
        // 80.6 without, 74.6 with, against 73.1 for the direct view.
        float3 wide = sampleMapLevel(m.maps.x, projected.xz * MACRO_SCALE, lod).rgb;
        float luma = dot(wide, float3(0.2126, 0.7152, 0.0722));
        albedo *= max((luma - MACRO_PIVOT) * MACRO_STRENGTH + 1.0, 0.0);
    }
    return albedo;
}

/// Triplanar normal mapping, without a tangent frame at all.
///
/// **The cotangent frame cannot be used here, and using it was a bug.** That
/// construction recovers a tangent basis from how position and UV change
/// across the quad. For triplanar there is no single UV — there are three
/// projections — so the earlier code fed it one of them, the XY plane. On flat
/// ground `worldPos.y` is constant, so both derivatives of that coordinate are
/// zero, the bitangent collapses, and the frame degenerates. The result is the
/// perturbed normal shooting off in one direction across whole regions of the
/// surface: hard vertical banding and long smears, worst on the flat meadows
/// and steep faces that make up most of a terrain.
///
/// Whiteout blending is the standard answer and needs no basis. Each of the
/// three sampled normals is treated as a perturbation of the world axis its
/// projection looks down, the surface normal is folded into its tangent
/// components, and the three are blended by the same weights the albedo uses.
/// Cheap, stable at every orientation, and there is no frame to go singular.
float3 triplanarNormal(uint index, float3 position, float3 geometric) {
    float3 weight = pow(abs(geometric), 4.0);
    weight /= max(weight.x + weight.y + weight.z, 1e-4);

    // **The X plane is `position.zy`.** This is Golus's whiteout construction
    // and it is not free to choose: the last line routes `nx.x` into world Z
    // via `.zyx`, so `nx.x` has to BE the z-perturbation. Sampled `.yz`, `nx.x`
    // is the y-perturbation and the X-facing normal comes out transposed --
    // which is what it was, on every +/-X flank in every scene, invisibly,
    // because no golden reference sampled a texture on voxel terrain.
    float3 nx = sampleMap(index, position.zy).xyz * 2.0 - 1.0;
    float3 ny = sampleMap(index, position.xz).xyz * 2.0 - 1.0;
    float3 nz = sampleMap(index, position.xy).xyz * 2.0 - 1.0;

    // Fold the surface normal into each sample's tangent plane, and keep the
    // sample's Z positive so a projection looking down the negative side of an
    // axis does not invert.
    nx = float3(nx.xy + geometric.zy, abs(nx.z) * geometric.x);
    ny = float3(ny.xy + geometric.xz, abs(ny.z) * geometric.y);
    nz = float3(nz.xy + geometric.xy, abs(nz.z) * geometric.z);

    // Swizzled back so each contribution is expressed in world axes.
    return normalize(nx.zyx * weight.x + ny.xzy * weight.y + nz.xyz * weight.z);
}

/// A tangent frame built from screen-space derivatives, with no stored tangents.
///
/// The mesh carries position, normal and UV — no tangent and no bitangent. The
/// frame is recovered per pixel from how position and UV change across the
/// quad, which is the Schüler/Mikkelsen construction.
///
/// Two reasons this is the right trade here rather than a compromise:
///
/// - It saves 8-12 bytes on every vertex in the scene and removes the entire
///   tangent-generation stage from the import pipeline.
/// - Triplanar geometry has no UVs and therefore no tangents to store, so a
///   stored-tangent path would need this code anyway for half the scene.
///
/// The cost is a handful of ALU per pixel, paid only where a normal map is
/// actually bound. On this hardware that is the cheaper side to spend.
float3x3 cotangentFrame(float3 normal, float3 position, float2 uv) {
    float3 dp1 = ddx(position);
    float3 dp2 = ddy(position);
    float2 duv1 = ddx(uv);
    float2 duv2 = ddy(uv);

    float3 dp2perp = cross(dp2, normal);
    float3 dp1perp = cross(normal, dp1);
    float3 tangent = dp2perp * duv1.x + dp1perp * duv2.x;
    float3 bitangent = dp2perp * duv1.y + dp1perp * duv2.y;

    // Scale by the largest component rather than normalising each axis: that
    // keeps the frame orthogonal where the UVs are stretched, instead of
    // silently correcting a stretch the artist put there on purpose.
    // Guarded: a surface whose UVs do not vary along one axis — a wall of a
    // box seen edge-on, an unwrapped seam — makes one of these vanish, and an
    // unguarded `rsqrt` of zero is an infinity that propagates into the normal.
    float extent = max(dot(tangent, tangent), dot(bitangent, bitangent));
    if (extent < 1e-12) {
        return float3x3(float3(1, 0, 0), float3(0, 1, 0), normal);
    }
    float invmax = rsqrt(extent);
    return float3x3(tangent * invmax, bitangent * invmax, normal);
}

/// GGX specular, the standard Trowbridge-Reitz distribution with a Smith
/// height-correlated visibility term and a Schlick Fresnel.
///
/// One highlight term rather than a full IBL: there is no reflection probe in
/// this engine yet, so anything more would be inventing detail the scene does
/// not contain.
float3 specular(float3 normal, float3 view, float3 light, float roughness, float3 f0) {
    // Not named `half`: that is a built-in scalar type in this language, and
    // shadowing it compiles today and stops compiling on a compiler update.
    float3 halfway = normalize(view + light);
    float nDotH = max(dot(normal, halfway), 0.0);
    float nDotV = max(dot(normal, view), 1e-4);
    float nDotL = max(dot(normal, light), 0.0);
    float vDotH = max(dot(view, halfway), 0.0);

    // Clamped away from zero: a perfectly smooth surface makes the
    // distribution a delta function, which is a division by zero on the GPU
    // and a NaN that spreads across the image.
    float a = max(roughness * roughness, 1e-3);
    float a2 = a * a;

    float d = nDotH * nDotH * (a2 - 1.0) + 1.0;
    float distribution = a2 / max(3.14159265 * d * d, 1e-6);

    float k = a * 0.5;
    float visibility = 0.5 / max(lerp(nDotV, 1.0, k) * lerp(nDotL, 1.0, k) * 4.0, 1e-4);

    float3 fresnel = f0 + (1.0 - f0) * pow(1.0 - vDotH, 5.0);
    return distribution * visibility * fresnel;
}

/// The scene's terrain height grid, assembled from the two environment fields
/// that carry it. `side == 0` is a scene with no terrain: every lookup then
/// returns `LOOM_NO_GROUND` and the water is bottomless.
///
/// **Up here rather than beside the water**, which is where it started and
/// where its only caller used to be. The wetness gate below reads the same grid
/// — one bake, three readers, and the third one must not grow a fourth.
LoomHeightField terrainField() {
    LoomHeightField field;
    field.origin = push.environment[0].terrain.xy;
    field.spacing = push.environment[0].terrain.z;
    field.side = int(push.environment[0].terrain.w);
    field.height = push.environment[0].terrainHeights;
    return field;
}

/// This tick's wavelet events, assembled from the two environment fields that
/// carry them. `count == 0` is water nothing has touched: every lookup returns
/// zero, which is the plain Gerstner surface bit for bit.
///
/// **The pool is the CPU's** — ADR 0056. It is stepped inside the fixed step
/// beside `rapier`, it is what buoyancy reads through `sample_water`, and this
/// is a copy uploaded for drawing. Nothing here writes it.
LoomWaveletPool waveletPool() {
    LoomWaveletPool pool;
    pool.events = push.environment[0].waveletEvents;
    pool.count = int(push.environment[0].wavelet.x);
    return pool;
}

/// This scene's river current, assembled from the two environment fields that
/// carry it. `side < 2` is water with no current: every lookup returns exactly
/// zero, and every term that reads it then takes the branch that leaves the
/// picture bit for bit as it was.
///
/// **The grid is the CPU's** — it is baked from the bed at load and it is what
/// `loom_water::flow::FlowGrid::at` hands the buoyancy solver. This is a copy
/// uploaded for drawing, and what it draws is where the surface *texture*
/// travels. It never reaches `WaterSample`, so `river`'s determinism hash is
/// unmoved by everything downstream of here.
LoomFlowGrid flowField() {
    LoomFlowGrid grid;
    grid.origin = push.environment[0].flow.xy;
    grid.spacing = push.environment[0].flow.z;
    grid.side = int(push.environment[0].flow.w);
    grid.velocity = push.environment[0].flowVelocity;
    return grid;
}

/// This tick's foam field, assembled from the environment fields that carry it.
/// `side == 0` is water with no field, and every lookup returns zero — which
/// leaves the closed-form trail as the only foam.
///
/// **The field is the CPU's** — ADR 0055. It is advected and deposited into
/// inside the fixed step, `water@x,z.foam` reads the same numbers through the
/// Rust half, and this is a copy uploaded for drawing. Nothing here writes it.
LoomFoamField foamField() {
    LoomFoamField field;
    field.origin = push.environment[0].foam.xy;
    field.cell = push.environment[0].foam.z;
    field.side = int(push.environment[0].foam.w);
    field.edge_cells = push.environment[0].foamEdgeCells;
    field.coverage = push.environment[0].foamCoverage;
    return field;
}

/// This tick's FFT cascade, assembled from the environment fields that carry it
/// — ADR 0076. `count == 0` is water that is not a spectrum sea, and
/// `loom_ocean_at` then returns the still surface.
///
/// **The tiles are the CPU's**, exactly as the foam field above is: they are
/// transformed inside the fixed step, buoyancy pushes bodies with them, and
/// `loom water --at` prints the same numbers through the Rust half. Nothing here
/// writes them, and nothing here reads one back — ADR 0045 clause 2.
LoomOcean oceanCascade() {
    LoomOcean sea;
    sea.patch = push.environment[0].oceanPatch;
    sea.n = int(push.environment[0].ocean.y);
    sea.count = int(push.environment[0].ocean.x);
    sea.tiles = push.environment[0].oceanTiles;
    return sea;
}

// ---------------------------------------------------------------------------
// Two-phase flow maps (Vlachos, Portal 2 / GDC 2010).
//
// **The problem being solved is that advecting a noise field by a spatially
// varying velocity folds it.** `xz - flow(xz)·t` is a valid warp only while
// `t·∂flow` stays under the pattern's own wavelength; a river's shear across a
// bank reaches that in a couple of seconds and the texture smears into streaks
// that keep stretching forever. The capillary term above already refuses a
// per-pixel drift for exactly this reason and settles for a global one.
//
// The fix is to bound `t`. Two copies of the pattern run the same warp on a
// sawtooth of period `WATER_FLOW_PERIOD`, half a cycle out of phase, and each
// is cross-faded to nothing at the instant it resets. The warp never exceeds
// one period's worth of displacement, so it never folds — and because the two
// weights sum to exactly one, the blend has no visible pulse at the boundary.
//
// **`w = 1 − |1 − 2p|` is what makes the sum exact.** At `p` and `p + 0.5` the
// two weights are `1 − |1 − 2p|` and `|1 − 2p|`, which add to one for every `p`
// in `[0, 1)`. A cosine window looks the same in a still and does not, so the
// brightness would breathe once per period — invisible in a PNG and the first
// thing a fly-through shows.
// ---------------------------------------------------------------------------

/// Seconds in one flow-map cycle.
///
/// **The one knob, and it is a trade between two artifacts.** Long, and the
/// warp gets far enough to smear: at `river`'s 3 m/s a 2 s period displaces the
/// pattern 6 m, which is ten lattice cells of the capillary noise. Short, and
/// the cross-fade is always half-way, which reads as a soft double image
/// rather than as moving water. 2.0–2.5 s is the usable band; 2.0 is chosen
/// because this river is fast and the smear is the worse of the two.
static const float WATER_FLOW_PERIOD = 2.0;

/// Where to sample the two phases of a flow map, and what the first is worth.
struct LoomFlowPhase {
    float2 uvA;
    float2 uvB;
    /// The second phase is worth exactly `1 - wA`. Stated once rather than
    /// stored twice, because the identity is the whole reason there is no
    /// pulse.
    float wA;
};

LoomFlowPhase waterFlowPhase(float2 xz, float2 flow, float t) {
    float pa = frac(t / WATER_FLOW_PERIOD);
    float pb = frac(t / WATER_FLOW_PERIOD + 0.5);
    LoomFlowPhase p;
    p.uvA = xz - flow * (pa * WATER_FLOW_PERIOD);
    p.uvB = xz - flow * (pb * WATER_FLOW_PERIOD);
    p.wA = 1.0 - abs(1.0 - 2.0 * pa);
    return p;
}

// ---------------------------------------------------------------------------
// Wetness.
//
// **Phase 4 step 3, and it is entirely a shading effect on top of two numbers
// the CPU decided.** `loom_rain::wetness` is the model: a water FILM that
// builds in seconds and drains in half a minute, and a SOAK that takes a minute
// to get into the material and five to leave it. The shader never integrates
// anything, never writes anything, and — like the streaks — is not in the
// determinism hash.
//
// The two numbers are for a surface in the OPEN. What varies across the scene
// is decided here, and it is decided out of the terrain height field the rain
// drops already read (never a second occlusion representation, ADR 0014):
//
//   * a point below the topmost solid surface in its column is under cover;
//   * a surface facing down does not collect rain, one facing up collects all
//     of it, and a vertical wall collects the fraction wind drives onto it.
//
// **What this cannot express**, stated rather than discovered later: the height
// field flattens an overhang away, so a wall under an eave is dry over its whole
// height rather than dry at the top and wet at the bottom, and mesh geometry
// shelters nothing at all. Neither can it express history — a floor uncovered
// after ten dry minutes is instantly as wet as the field outside, because there
// is one pair of scalars for the scene rather than a wetness grid with an update
// path. That grid is the upgrade, and it costs storage and a write path.
// `ponytail:` one global pair + a per-pixel gate; a wetness texture if a scene
// ever needs a surface to remember.
// ---------------------------------------------------------------------------

/// The water film, and the soak. `loom_rain::wetness` at full exposure, packed
/// into the two free trailing scalars of the environment buffer.
float wetFilm() { return push.environment[0].weather.w; }
float wetSoak() { return push.environment[0].water.w; }

/// Porosity for geometry with no `Material` — a blockout, drawn in its debug
/// colour. The same value as `loom_scene::components::Material::porosity`'s
/// default, so texturing a box does not change how wet it looks.
static const float WET_BLOCKOUT_POROSITY = 0.4;
/// How dark a fully porous, fully soaked surface goes: down to a third.
///
/// Calibrated by eye against wet concrete, which is what the literature does
/// too — Lagarde's own note is that the per-channel physical curves are not
/// worth their cost over one monochrome factor.
static const float WET_DARKEN = 0.7;
/// Roughness a water film converges on. Not zero: even standing water has a
/// little texture, and a true mirror in a scene with no reflections reads as a
/// hole.
static const float WET_SMOOTH = 0.08;
/// Fraction of the rain a vertical wall collects. Rain arrives from above, so
/// a wall only gets what the wind drives onto it — but zero would leave a
/// soaked courtyard ringed by bone-dry walls, which reads as a bug.
static const float WET_WALL = 0.35;
/// Metres of slack in the "is this under cover" test.
///
/// The height field is a bilinear read of a grid coarser than a fragment, so a
/// surface lying exactly ON the terrain sits within a few centimetres of its own
/// column height either way. Without slack the ground flickers between wet and
/// dry across a triangle. Half a metre is far below any roof worth sheltering
/// under.
static const float WET_COVER_SLACK = 0.5;

/// How much of the scene's wetness reaches this surface point.
///
/// The geometric normal, deliberately — a normal map should change how a wet
/// surface catches the light, not whether it is wet.
/// Seconds of history the wetness cover average looks back over, and how many
/// samples it takes. **Mirrors `loom_rain::WET_COVER_WINDOW` and
/// `WET_COVER_TAPS`** — a hand-written twin, because the averaging is a loop
/// over `clouds_at` and S2's expression tree expresses scalar fields rather
/// than loops over one. These two numbers are the whole shared contract.
static const float WET_COVER_WINDOW = 40.0;
static const int WET_COVER_TAPS = 3;

/// How much cloud has been over this point lately, 0 to 1.
///
/// **Not the cover now, and that is the point.** Wetness is the integral of
/// rain over the last minute; a deck drifting at 17 m/s crosses a thirty-metre
/// scene in two seconds. Gating wetness on the instantaneous value would make
/// the ground brighten and darken as cloud shadows pass, which is what *lit*
/// ground does and not what *wet* ground does.
float cloudCoverRecent(float3 world) {
    // A solid deck is over everywhere, always. This is what keeps every scene
    // that authors no cover — which is every scene that rains, by default —
    // paying nothing for three `clouds_at` evaluations per ground pixel.
    if (push.environment[0].cloud.x >= 1.0) {
        return 1.0;
    }
    if (push.environment[0].cloud.x <= 0.0) {
        return 0.0;
    }
    float now = push.environment[0].weather.z;
    float total = 0.0;
    for (int i = 0; i < WET_COVER_TAPS; ++i) {
        float back = WET_COVER_TAPS > 1
            ? WET_COVER_WINDOW * float(i) / float(WET_COVER_TAPS - 1)
            : 0.0;
        total += cloudCoverAtTime(world, max(now - back, 0.0));
    }
    return total / float(WET_COVER_TAPS);
}

float wetGate(float3 worldPos, float3 geometric) {
    // Off the grid, and in a scene with no terrain, this is a large negative
    // sentinel and nothing is ever under cover.
    float ground = loom_ground_height(terrainField(), worldPos.xz);
    float open = worldPos.y > ground - WET_COVER_SLACK ? 1.0 : 0.0;
    float facing = smoothstep(-0.3, 0.0, geometric.y)
        * lerp(WET_WALL, 1.0, saturate(geometric.y));
    // **And the cloud that has been over it**, which is the third of the three
    // things that decide whether a surface is wet — the other two being the
    // roof above it and which way it faces. The film and soak this gates are
    // still one pair of scalars for the scene, computed from the *unsheltered,
    // uncovered* rate; this is where both per-position terms are applied, the
    // same division of labour the streaks follow.
    return open * facing * cloudCoverRecent(worldPos);
}

// ---------------------------------------------------------------------------
// Puddles — Phase 4's last shading item.
//
// **Standing water where the ground holds it, from the shape of the ground.**
// The implementation order asks for `loom_terrain` flow accumulation gated by
// slope and curvature, and warns in the same sentence that accumulation alone
// marks drainage *convergence* rather than flat basins, so ungated puddles
// climb gentle inclines.
//
// There is no drainage field for a voxel volume. `GroundGrid` says so where
// grass reads the same ground: real flow means routing water downhill across
// the whole field and counting what drains through each cell, and
// `loom_terrain` is a separate heightmap system these scenes do not use. So
// **concavity stands in for accumulation** — the same substitution grass
// already makes, for the same reason — and the slope gate is exactly the one
// that warning asks for. Replace both with real accumulation when erosion lands
// and there is a drainage field worth reading.
//
// **Rendering-only, and outside the determinism hash**, the same exemption
// grass and the rain streaks have. Nothing reads a puddle back: it is a term in
// the ground's shading, not a body of water. That is also its honest limit —
// see `PUDDLE_FULL`.
// ---------------------------------------------------------------------------

/// How far above the still-water line the sand still reads as wet, in metres.
/// The swash reaches further than the mean surface and drains back slowly.
static const float WET_SAND_REACH = 0.55;
/// How much saturated sand darkens. High: dry sand to wet sand is one of the
/// largest albedo changes any common material makes.
static const float WET_SAND_DARKEN = 0.55;
static const float WET_SAND_SMOOTH = 0.35;

/// How smooth standing water is. Below `WET_SMOOTH`, because a puddle is water
/// rather than a wet surface — the film smooths what is under it, this replaces
/// it.
static const float PUDDLE_ROUGHNESS = 0.03;

/// How much a puddle darkens what it covers, on top of the soak.
///
/// Water is nearly clear; what makes a puddle dark is that most of the light
/// reaching your eye from it is reflected sky rather than scattered ground, and
/// the specular lobe supplies that separately. This is the other half.
static const float PUDDLE_DARKEN = 0.45;

/// Metres the ground must dish over `PUDDLE_REACH` to read as a full puddle.
///
/// **Small on purpose.** A hollow deep enough to be obvious is a pond, and a
/// pond wants a `WaterBody` with a surface height, buoyancy and a shoreline —
/// all of which exist. What has no representation until now is the two
/// centimetres of water lying in a wheel rut, which is what a wet road is
/// mostly made of.
static const float PUDDLE_FULL = 0.16;

/// Radius the concavity is measured over, in metres.
static const float PUDDLE_REACH = 1.1;

/// Slope, as a gradient magnitude, over which no water stands at all.
///
/// **This is the gate the design warning is about.** Concavity alone is happily
/// positive on the inside of a bend halfway up a hill, and water does not
/// collect there. 0.12 is about seven degrees.
static const float PUDDLE_LEVEL = 0.04;
static const float PUDDLE_SLOPE = 0.12;

/// How much of the surface is standing water here, 0 to 1.
float puddleMask(float3 worldPos) {
    LoomHeightField field = terrainField();
    if (field.side <= 0) {
        return 0.0;
    }
    float2 xz = worldPos.xz;
    float h = loom_ground_height(field, xz);
    float x0 = loom_ground_height(field, xz - float2(PUDDLE_REACH, 0.0));
    float x1 = loom_ground_height(field, xz + float2(PUDDLE_REACH, 0.0));
    float z0 = loom_ground_height(field, xz - float2(0.0, PUDDLE_REACH));
    float z1 = loom_ground_height(field, xz + float2(0.0, PUDDLE_REACH));
    // **Any absent neighbour disqualifies the whole sample.** Off the grid the
    // query returns -1e9, and a mean containing one of those is a hollow half a
    // billion metres deep — the edge of every terrain would flood.
    float lowest = min(min(h, min(x0, x1)), min(z0, z1));
    if (lowest < -1.0e8) {
        return 0.0;
    }

    // Positive is a hollow. A ridge is negative and clamps away.
    float hollow = (x0 + x1 + z0 + z1) * 0.25 - h;
    float2 gradient = float2(x1 - x0, z1 - z0) / (2.0 * PUDDLE_REACH);
    float flat = 1.0 - smoothstep(PUDDLE_LEVEL, PUDDLE_SLOPE, length(gradient));

    return saturate(hollow / PUDDLE_FULL) * flat;
}

/// Rate, in mm/h, at which the visual layer is at full strength. Light rain is
/// both sparser and fainter, and nothing past this rate gets denser or
/// brighter.
///
/// **Both, rather than only the density, on purpose.** Exposure at the eye
/// moves continuously as a camera walks under a ledge; dropping cells alone
/// would step the layer out one drop at a time, and a fade is what makes that a
/// transition rather than a stutter.
///
/// **It is spelled once.** The Rust side used to hold the same 20.0 and scale
/// the draw count by it; a grid cannot be thinned by shortening the draw, so
/// both halves moved into this file. It sits up here rather than beside the
/// streaks — where it was, and where its only reader used to be — because the
/// ripples below thin themselves against exactly the same number, and rain that
/// was falling harder than it was rippling would be two answers to one
/// question. `loom_rain`'s `SPLASH_FULL_RATE` is the third reader, and the one
/// that cannot be shared: it is on the far side of the CPU boundary.
///
/// **It moved again, to `include/rain.slang`**, when the drop simulation became
/// the fourth reader — `rain_sim.slang` is a separate module and could not see
/// a constant declared here.

// ---------------------------------------------------------------------------
// Ripples — Phase 4 step 4's shading half.
//
// **Expanding rings on a wet surface, and they are what reads as rain landing.**
// Streaks over a wet-but-glassy floor look like rain in front of a photograph
// of rain; the rings are what put the two in the same world.
//
// Procedural rather than a scrolling normal atlas, which is what the research
// pass proposes ("ripple normal atlas first"). Same cost class and one fewer
// asset: no texture to author, no descriptor, no upload path, no tiling period,
// and the scale is in metres rather than in whatever the atlas was drawn at.
// Cyanilux's rain breakdown does the same thing for the same reason.
//
// **Where they appear is [`wetGate`]'s answer and no other**: a surface under
// cover has no rings for the same reason it is not wet, out of the same terrain
// height field the drops are culled against. How many appear is
// `environment.rain.w` — the scene's unsheltered rate — against the same
// `RAIN_FULL_RATE` the streaks thin themselves by. Unsheltered is right here for
// exactly the reason `wetFilm` is: how hard it is raining is one number for the
// scene, and WHERE that reaches is `wetGate`'s per-pixel answer. This shader
// decides *where a ring is*, and nothing else.
//
// Nothing here is state. A ring's radius is a function of the clock and its
// cell, so there is no ripple buffer, nothing to step, and nothing that could
// be read back into the simulation.
// ---------------------------------------------------------------------------

/// Metres per ripple cell. One ring at a time in each, which is what buys the
/// single hash per fragment below.
static const float RIPPLE_CELL = 1.0;
/// Seconds between one cell's rings.
static const float RIPPLE_PERIOD = 0.9;
/// Seconds a ring lives. Shorter than the period, so a cell is empty part of
/// the time and the field does not read as a permanent lattice of rings.
static const float RIPPLE_LIFE = 0.55;
/// Metres a ring reaches by the end of its life.
///
/// **Plus [`RIPPLE_WIDTH`] it must stay under a quarter of a cell**, because the
/// centre is confined to the middle half of one and a ring that crossed the
/// boundary would be clipped by a straight line. That bound is what keeps this
/// to one hash per fragment instead of nine.
static const float RIPPLE_RADIUS = 0.16;
/// Metres across the crest of the ring.
static const float RIPPLE_WIDTH = 0.04;
/// How far the crest tilts the normal.
static const float RIPPLE_STRENGTH = 0.4;

/// The surface normal, perturbed by whatever ring is passing through this point.
///
/// `wet` is [`wetGate`] — cover, and how squarely the surface faces up. Zero
/// under a roof, so the rings stop exactly where the wetness does.
float3 rippleNormal(float3 worldPos, float3 normal, float wet) {
    float rate = saturate(push.environment[0].rain.w / RAIN_FULL_RATE);
    float strength = rate * wet;
    if (strength <= 0.0) {
        return normal;
    }

    const float SCALE = 1.0 / 16777216.0;
    float2 p = worldPos.xz / RIPPLE_CELL;
    int2 cell = int2(floor(p));
    float2 inCell = p - float2(cell);

    // `loom_hash` again — the project's one integer hash, folded over the
    // coordinates the way `rainHash` and `loom_lattice` fold theirs.
    uint seed = loom_hash(loom_hash(uint(cell.x)) + uint(cell.y));
    // Phased per cell, so neighbouring rings do not appear in unison.
    float phase = float(seed >> 8) * SCALE;
    float s = push.environment[0].weather.z / RIPPLE_PERIOD + phase;
    float cycle = floor(s);
    float age = (s - cycle) * RIPPLE_PERIOD;
    if (age >= RIPPLE_LIFE) {
        return normal;
    }

    // **Rehashed per ring**, so the next one in this cell lands somewhere else.
    // Hashing the centre from the cell alone makes every cell a tap dripping on
    // one spot, and the grid becomes visible within two seconds.
    uint h = loom_hash(seed + uint(int(cycle)));
    uint h2 = loom_hash(h);
    uint h3 = loom_hash(h2);
    // Light rain rings in fewer places as well as more faintly — the same
    // per-cell thinning against the same ratio the drops use.
    if (float(h3 >> 8) * SCALE >= rate) {
        return normal;
    }

    float2 centre = float2(0.25 + 0.5 * float(h >> 8) * SCALE,
                           0.25 + 0.5 * float(h2 >> 8) * SCALE);
    float2 offset = (inCell - centre) * RIPPLE_CELL;
    float dist = length(offset);
    float radius = (age / RIPPLE_LIFE) * RIPPLE_RADIUS;

    // The *slope* of the ring, which is what a normal is: the derivative of a
    // gaussian crest. Positive on the way up and negative on the way down, so
    // the crest catches the light on one side and loses it on the other, which
    // is what makes a ring read as raised rather than as a painted circle.
    float x = (dist - radius) / RIPPLE_WIDTH;
    float slope = -2.0 * x * exp(-x * x);
    // Dying as it spreads, like a real one: the energy is fixed and the
    // circumference is not.
    float fade = 1.0 - age / RIPPLE_LIFE;
    float2 dir = dist > 1e-4 ? offset / dist : float2(0.0, 0.0);
    float3 tilt = float3(dir.x, 0.0, dir.y) * (slope * RIPPLE_STRENGTH * fade * strength);
    return normalize(normal + tilt);
}

/// How much of the steep-slope layer shows here, 0 to 1.
///
/// **The threshold wanders, and that is the whole difference between a
/// hillside and a shaved ring.** A cutoff on slope — at any value, softened by
/// any amount — draws the boundary along a clean curve across a smooth hill,
/// and a clean curve is the synthetic tell. `loom_grass::coverage` learned this
/// the hard way and perturbs its *threshold* with low-frequency noise on world
/// position; widening the fade band instead only blurs the curve, which was
/// tried there first and looked exactly as artificial.
///
/// The constants below are `loom_grass::coverage`'s, spelled again because a
/// shader cannot read a Rust constant. **They must stay equal**: grass stopping
/// at one angle while the rock beneath it starts at another draws two
/// concentric rings around the same hill, which is worse than either boundary
/// on its own.
///
/// **Sampled in 2D, exactly as `coverage` does**, so the two agree; and then
/// perturbed a second time in full 3D, which `coverage` does not need and this
/// does. A 2D wander is constant all the way down a vertical face, so on a
/// cliff it stops varying at precisely the place the layer is most visible.
/// The 3D term is additive rather than a threshold shift, which is what makes
/// rock appear *in places* instead of as a band at one height.
static const float LAYER_BREAKUP = 0.14;
static const float LAYER_BREAKUP_SCALE = 0.32;
static const float LAYER_FADE = 0.15;
static const float LAYER_PATCH_SCALE = 0.067;
static const float LAYER_PATCH = 0.6;
float groundLayerWeight(float3 p, float3 geometric, float cutoff) {
    float wander = loom_value_noise(float3(p.x, 0.0, p.z) * LAYER_BREAKUP_SCALE) * LAYER_BREAKUP;
    // Inverted against `coverage`: grass fades OUT as the ground steepens past
    // the cutoff, so the rock underneath fades IN over the same band.
    float w = 1.0 - saturate((geometric.y - cutoff - wander) / LAYER_FADE);
    // **Scaled by the slope, so dead-flat ground is soil by construction.**
    // Ungated, the patch term put up to 30% rock across a level field and
    // washed it grey — which is not "outcrops in places", it is haze. `steep`
    // is 0 where the ground is level and 1 at the cutoff angle, so patches
    // emerge as the ground tilts and the flats stay clean. It is the same
    // shape of guarantee `loom_grass` makes when it lets its wander only ever
    // subtract: the parameter keeps its documented meaning at the extremes.
    float steep = saturate((1.0 - geometric.y) / max(1.0 - cutoff, 1e-3));
    float patch = (loom_value_noise(p * LAYER_PATCH_SCALE) - 0.5) * LAYER_PATCH * steep;
    return saturate(w + patch);
}

[shader("fragment")]
float4 fragmentMain(VSOutput in) : SV_Target {
    // One directional sun plus ambient from the sky. The ambient used to be a
    // flat 0.25, which lit the shadowed side of everything with the same grey
    // whatever it faced; sampling the sky gradient instead means a face turned
    // upward picks up blue and one turned down picks up ground.
    float3 normal = normalize(in.normal);
    float3 albedo = in.color;
    float metallic = 0.0;
    float roughness = 0.8;
    float porosity = WET_BLOCKOUT_POROSITY;

    // The unmapped normal. Up here because the wet block below needs it, the
    // ground layer below needs it to know the slope, and because the wet block
    // *changes the normal* — see the note on the sun term underneath it.
    float3 geometric = normalize(in.normal);

    // **Could this fragment have been killed by the alpha cutout below?**
    // `ambientVisibility` reads the other three lanes of the 2x2, and a quad
    // read from a lane that executed `discard` is undefined — Slang lowers it
    // to `OpKill`, not to a demote. Both terms of this are *material*
    // constants and `material` is `nointerpolation`, so a quad, being one
    // primitive, is one material and every lane agrees: a quad that could
    // contain a kill simply does not take the sharing path. That is why this
    // needs no `shaderDemoteToHelperInvocation` and no `slangc` capability —
    // the control flow already excludes the hazard. `fragmentMain` has exactly
    // one `discard`, and this is its condition.
    //
    // What it costs is that alpha-cut foliage keeps the unshared eight rays,
    // at today's price. Two scenes in the repository pay it.
    bool cutoutMaterial = false;

    if (in.material != NO_TEXTURE) {
        MaterialData m = push.materials[in.material];
        albedo = m.albedo.rgb;
        metallic = m.params.x;
        roughness = m.params.y;
        porosity = m.albedo.w;

        bool projected = (m.maps.z & FLAG_TRIPLANAR) != 0;
        // Scaling the *coordinate* rather than the sample: for triplanar this
        // is world position, so a shared texture keeps a consistent real-world
        // size across every object that uses it however each one is scaled.
        float2 uv = in.uv * m.params.zw;
        float3 projectedUv = in.worldPos * float3(m.params.z, m.params.w, m.params.z);

        if (m.maps.x != NO_TEXTURE) {
            float4 sampled = projected
                ? triplanar(m.maps.x, projectedUv, normal)
                : sampleMap(m.maps.x, uv);

            // **Alpha cutout, and it has to come first.** A leaf is a shape cut
            // out of a rectangle; without this the rectangle draws. The branch
            // is on a threshold of exactly zero so that every material which
            // does not author one costs a compare and nothing else — and so
            // that a texture with no authored alpha can never erase itself.
            cutoutMaterial = m.meanAlbedo.w > 0.0;
            if (cutoutMaterial && sampled.a < m.meanAlbedo.w) {
                discard;
            }

            // Multiplied, not replaced: the albedo factor stays useful as a
            // tint, which is how one texture dresses several objects.
            albedo *= sampled.rgb;

            if (projected) {
                // One extra sample, top-down only. The vertical projection is
                // enough because this varies over tens of metres and nobody
                // reads large-scale colour off a cliff face; paying for three
                // would triple the cost of the cheapest half of the effect.
                float3 wide = sampleMap(m.maps.x, projectedUv.xz * MACRO_SCALE).rgb;
                float luma = dot(wide, float3(0.2126, 0.7152, 0.0722));
                albedo *= max((luma - MACRO_PIVOT) * MACRO_STRENGTH + 1.0, 0.0);
            }
        }

        if (m.maps.y != NO_TEXTURE) {
            if (projected) {
                // No tangent frame: triplanar geometry has no UVs to derive
                // one from, and deriving it from one of the three projections
                // degenerates wherever that projection is edge-on.
                normal = triplanarNormal(m.maps.y, projectedUv, normal);
            } else {
                float3 tangentNormal = sampleMap(m.maps.y, uv).xyz * 2.0 - 1.0;
                float3x3 frame = cotangentFrame(normal, in.worldPos, uv);
                normal = normalize(mul(tangentNormal, frame));
            }
        }

        // -------------------------------------------------------------------
        // **The steep-slope layer: what stops terrain being one substance.**
        //
        // `m.maps.w` indexes a second, complete `MaterialData` — its own
        // textures, roughness, porosity and uv_scale — shown where the ground
        // is too steep for the first to lie on it. A colour would not do: rock
        // is smoother than soil and holds far less water, and blending those
        // is most of what makes the two read as different substances rather
        // than one substance painted twice.
        //
        // **The branch is wave-uniform.** `in.material` is `nointerpolation`,
        // so every lane in a wave reads the same `maps.w` and a scene with no
        // layer pays nothing — not an identity multiply, nothing.
        // -------------------------------------------------------------------
        if (m.maps.w != NO_TEXTURE) {
            MaterialData lm = push.materials[m.maps.w];
            bool lproj = (lm.maps.z & FLAG_TRIPLANAR) != 0;
            float2 luv = in.uv * lm.params.zw;
            float3 lprojUv = in.worldPos * float3(lm.params.z, lm.params.w, lm.params.z);

            float3 lalbedo = lm.albedo.rgb;
            if (lm.maps.x != NO_TEXTURE) {
                lalbedo *= (lproj ? triplanar(lm.maps.x, lprojUv, geometric)
                                  : sampleMap(lm.maps.x, luv)).rgb;
            }
            float3 lnormal = geometric;
            if (lm.maps.y != NO_TEXTURE) {
                if (lproj) {
                    lnormal = triplanarNormal(lm.maps.y, lprojUv, geometric);
                } else {
                    float3 lt = sampleMap(lm.maps.y, luv).xyz * 2.0 - 1.0;
                    lnormal = normalize(mul(lt, cotangentFrame(geometric, in.worldPos, luv)));
                }
            }

            float w = groundLayerWeight(in.worldPos, geometric, lm.params.x);
            albedo = lerp(albedo, lalbedo, w);
            normal = normalize(lerp(normal, lnormal, w));
            roughness = lerp(roughness, lm.params.y, w);
            porosity = lerp(porosity, lm.albedo.w, w);
        }
    }


    // -----------------------------------------------------------------------
    // **Wet.** Two properties, driven by the two halves of the model, and they
    // are separate so that drying is not a crossfade: the film goes first and
    // takes the sheen with it, the darkening stays.
    //
    // Porosity is what makes darkening a property of the material rather than
    // of the weather — a polished stone barely darkens, a raw one goes almost
    // black — and metal is excluded because metal has no pores to fill.
    //
    // **Branched, and the branch is uniform over the whole draw**: both scalars
    // are per-frame constants, so a dry scene pays nothing for this and — more
    // to the point — reaches the lighting below with `albedo` and `roughness`
    // bit for bit what they were. Multiplying by an identity would very nearly
    // do that, and "very nearly" is what a golden image is there to catch.
    // -----------------------------------------------------------------------
    // Declared out here because the puddle's *reflection* has to be applied
    // after the lighting below, not before it. See the block near `lit`.
    // **Wet sand: the band the sea has just been over.**
    //
    // A beach is not one material. Below the swash line the sand is saturated
    // — darker, smoother, and it holds a sheen for a while after the water
    // has gone — and that band is most of what makes a waterline read as a
    // waterline rather than as a line where two things were authored.
    //
    // Driven by height above the still-water level, which is already in the
    // environment buffer, and reaching HIGHER than the water does: the sea
    // washes up the slope and drains back, and the sand stays wet in between.
    // That lag is the whole point — gating on where the water is *now* would
    // make the band flicker with the swell instead of tracking where it has
    // been, which is the same mistake the rain wetness made before it started
    // averaging cover over forty seconds.
    // **Caustics on ground that is under the water.** The water shader already
    // paints them on whatever it sees through the surface; this is the same
    // web on the same bed for the case where the eye is under there with it.
    // Without it, swimming down turns the seabed into flat sand while the
    // identical sand seen from above is dappled.
    //
    // **Only when the eye is under there with it, which is what the comment
    // above always said and the code did not.** Seen from above, the water
    // shader paints this exact web on this exact bed through `waterBehind` —
    // so an unguarded multiply here applied it twice, and a web squared is a
    // web with brighter filaments and darker cells than the one the surface
    // is focusing. It read as more caustic rather than as wrong, which is why
    // it survived a blessing.
    if (waterPresent() && eyeUnderwater()) {
        float submerged = stillWaterLevel() - in.worldPos.y;
        if (submerged > 0.0) {
            albedo *= 1.0 + causticWeb(in.worldPos, submerged);
        }
    }

    if (waterPresent() && porosity > 0.0) {
        float above = in.worldPos.y - stillWaterLevel();
        float wetBand = 1.0 - smoothstep(0.0, WET_SAND_REACH, above);
        // Rough edge, so the band is not a contour line drawn round the
        // island. Same reasoning as the grass slope boundary and the swash.
        wetBand *= saturate(0.65 + 0.5 * loom_value_noise(in.worldPos * 0.35));
        wetBand = saturate(wetBand);
        albedo *= 1.0 - WET_SAND_DARKEN * porosity * wetBand;
        roughness = lerp(roughness, min(roughness, WET_SAND_SMOOTH), wetBand);
    }

    float pool = 0.0;
    if (wetFilm() + wetSoak() > 0.0) {
        float wet = wetGate(in.worldPos, geometric);
        albedo *= 1.0 - WET_DARKEN * porosity * (1.0 - metallic) * wet * wetSoak();
        // Toward a water film, never rougher than the surface already is: a
        // material authored smoother than standing water stays that way.
        roughness = lerp(roughness, min(roughness, WET_SMOOTH), wet * wetFilm());
        // **And the rings, on the film rather than on the material.** Scaled by
        // the film because a ring is a disturbance *in* standing water: once the
        // sheen has drained there is nothing left to ripple, which is also what
        // stops them the moment a shower ends. Only where the surface faces up
        // — a ring running up a wall is not a thing rain does, and the tilt is
        // in the horizontal plane.
        // **Puddles, and they are the film's extreme rather than a new
        // system.** Water lies where the ground dishes and the slope is gentle,
        // and it only lies there once there is water to lie: the mask is scaled
        // by the same film that darkens everything else, so puddles fill as it
        // rains and drain when it stops without any state at all.
        //
        // `wet` gates it too, so nothing pools under a roof — the same answer
        // that stops the drops, out of the same height field.
        pool = puddleMask(in.worldPos) * wet * wetFilm();
        if (pool > 0.0) {
            // A puddle's surface is level whatever the ground under it does.
            // That flattening is most of what reads as standing water rather
            // than as a damp patch, because it is what lets a hollow reflect
            // the sky instead of the inside of itself.
            normal = normalize(lerp(normal, float3(0.0, 1.0, 0.0), pool));
            // Water is smoother than anything it sits on, and darker: what you
            // see is mostly the sky in it, which the specular lobe supplies.
            roughness = lerp(roughness, PUDDLE_ROUGHNESS, pool);
            albedo *= 1.0 - PUDDLE_DARKEN * pool;
        }
        // **And the rings, on the film rather than on the material.** Scaled by
        // the film because a ring is a disturbance *in* standing water: once the
        // sheen has drained there is nothing left to ripple, which is also what
        // stops them the moment a shower ends. Only where the surface faces up
        // — a ring running up a wall is not a thing rain does, and the tilt is
        // in the horizontal plane.
        //
        // **Strongest in the puddles**, which is where there is most water to
        // disturb — and where a ring has a flat surface to spread across rather
        // than a tilted one.
        normal = rippleNormal(
            in.worldPos,
            normal,
            wet * wetFilm() * saturate(geometric.y) * (1.0 + pool));
    }

    // **After the wet block, and that ordering is the whole feature.** These
    // three lines used to sit above it, which was harmless while the block only
    // touched albedo and roughness — and silently threw away most of the
    // ripples, which are a change to the *normal*: a ring reached the specular
    // lobe and the sky reflection, both of which read `normal` below, but not
    // the diffuse term or the sky ambient, which had already been computed off
    // the flat one. The measured effect was rings visible only inside the sun's
    // highlight, a few dozen pixels of a frame, and a golden image that could
    // not see the feature at all. A dry scene skips the block entirely, so this
    // is pure code motion for one and the difference between working and not
    // for the other.
    float diffuse = max(dot(normal, sunDirection()), 0.0);
    // **The sky ambient, and only it, is occluded.** `hemisphereAmbient` is the
    // sky arriving from the whole hemisphere above this surface, so "how much
    // of that hemisphere can actually be seen" is exactly the factor it is
    // missing — which is what makes AO a correction to this term rather than a
    // darkening applied to the finished pixel. The sun below has its own
    // visibility ray and does not want a second one, and the point lights after
    // it are at known positions, so occluding them with a hemisphere average
    // would be replacing a right answer with an approximate one.
    //
    // Traced against the *geometric* normal for the same reason the shadow is:
    // a normal map can tilt the hemisphere far enough to put half its rays
    // under the real surface, which reads as blotches of pure occlusion.
    float3 ambient = hemisphereAmbient(normal) * ambientStrength()
                   * ambientVisibility(in.rayOrigin, geometric, in.shadowBias, in.position.xy,
                                      !cutoutMaterial);
    // **Point lights, added to the ambient term.** They are unshadowed: a
    // shadowed point light needs a cube map or a ray per light per pixel, and
    // the ray tracer here is aimed at the sun. For a fire in the dark that is
    // the right trade — what a campfire mostly does is light everything near
    // it, and the shape it casts matters far less than that it lights at all.
    ambient += pointLights(in.worldPos, normal);
    // Only trace where the sun could reach anyway: a face turned away from the
    // sun is already dark, and tracing from it costs a ray to learn nothing.
    //
    // Traced against the *geometric* normal, not the mapped one: a normal map
    // can tilt a surface far enough to push the ray origin below the real
    // surface, and the ray then starts inside its own geometry.
    float sun = diffuse > 0.0
        ? sunVisibility(in.rayOrigin, geometric, in.shadowBias, in.position.xy)
        : 0.0;

    // Metals have no diffuse response — their albedo is the reflectance of the
    // specular lobe instead, which is why it moves into f0 rather than being
    // used twice.
    float3 diffuseColor = albedo * (1.0 - metallic);
    float3 f0 = lerp(float3(0.04), albedo, metallic);

    float3 lit = diffuseColor * (ambient + sunStrength() * diffuse * sun);
    float3 view = normalize(push.environment[0].eye.xyz - in.worldPos);
    lit += specular(normal, view, sunDirection(), roughness, f0) * sunStrength() * diffuse * sun;

    // **Without this a metal renders black.** Metals have no diffuse response,
    // so a fully metallic surface lit by one directional sun is unlit
    // everywhere except the highlight — physically correct and useless to look
    // at, because there is no environment for it to reflect.
    //
    // There is one: the sky this shader already draws. Reflecting it costs a
    // gradient evaluation and no texture, no probe and no extra pass, and it
    // is consistent with the background by construction rather than by tuning.
    float3 reflection = reflect(-view, normal);
    // A rough surface reflects a cone rather than a ray. Bending the direction
    // toward the normal as roughness rises is the usual cheap stand-in for a
    // prefiltered environment map, and the fade keeps a chalky surface from
    // picking up a mirror's worth of sky.
    float3 lobe = normalize(lerp(reflection, normal, roughness));
    // **And now the scene is in it too.** This was `skyGradient(lobe)` — an
    // analytic sky and nothing else, so a mirror in a room reflected the
    // weather through the ceiling. At roughness 1 it still is, bit for bit,
    // which is what keeps a fully matte surface unchanged.
    float3 environment = tracedEnvironment(in.rayOrigin, geometric, lobe, roughness,
                                           in.shadowBias, skyGradient(lobe));
    lit += environment * f0 * (1.0 - roughness * 0.7);

    // **A puddle is a mirror, and this is what was missing.** The term above is
    // a constant 4% of the sky — correct at normal incidence and wrong at every
    // other angle, because it carries no Fresnel. Water at a grazing angle
    // reflects nearly all of what hits it, and that is the entire reason a
    // puddle is visible on a wet road: not that it is darker, but that it is
    // full of sky.
    //
    // Without this, a puddle away from the sun's glint is a dark patch of
    // paint. It was shipped that way, and a human reported it as puddles being
    // invisible, which was the right description.
    //
    // **Schlick's, the same constants `waterFragmentMain` uses**, so a puddle
    // and the sea it is next to agree about what water does to light. And
    // `skyColor` rather than `skyGradient` — the reflection carries the clouds
    // and the sun, which is most of what makes it read as a reflection at all.
    if (pool > 0.0) {
        float grazing = saturate(1.0 - dot(normal, view));
        float fresnel = 0.02 + 0.98 * pow(grazing, 5.0);
        lit = lerp(lit, skyColor(reflection), fresnel * pool);
    }

    // Last, over the finished shading. Fog is what is between the eye and the
    // surface, so it replaces the surface's colour rather than joining the
    // lighting that produced it.
    float3 fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    lit = lerp(lit, fogColor(-view), fog);

    // **Opacity, and it is deliberately not applied to the colour.** The blend
    // is `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`, so the hardware multiplies; doing it
    // here as well would square it and glass would come out twice as dark as
    // authored. The opaque pipeline ignores this channel entirely, so every
    // material at the default 1.0 writes exactly what it wrote before.
    //
    // **Reflections stay at full strength while transmission fades**, because
    // `lit` already contains the specular and sky terms and they are not scaled
    // down here — which is what a real window does: at a glancing angle it is
    // almost a mirror, and the thing behind it disappears.
    float opacity = 1.0;
    if (in.material != NO_TEXTURE) {
        opacity = push.materials[in.material].misc.x;
    }
    return float4(lit, opacity);
}

// ---------------------------------------------------------------------------
// Particles.
//
// **Six vertices per particle and no vertex buffer.** The quad is built from
// SV_VertexID, the same way the sky's fullscreen triangle is: the instance
// data is read through a device address and the corners are arithmetic. That
// keeps the CPU cost of a plume at one draw call regardless of how many
// particles are in it.
//
// The billboard faces the *eye*, not the near plane. A plane-facing billboard
// is cheaper and is what most engines use, but it shears visibly when a plume
// sits near the edge of a wide field of view — and these renders are framed
// automatically, so things end up at the edge often.
// ---------------------------------------------------------------------------

/// Pixels a smeared droplet is never allowed to be narrower than.
///
/// **2.5, and the number is measured rather than chosen — see
/// `RAIN_MIN_PIXELS`, whose table this is taken from.** "At least one pixel" is
/// the conventional form of the trick and it is measurably the wrong setting: a
/// one-pixel primitive still straddles pixel boundaries, so it produces *more*
/// isolated pixels than no floor at all. On rain's own sky band, as a fraction
/// of pixels disagreeing sharply with their neighbourhood:
///
///     floor  0.0  0.75%      1.0  0.95%      1.5  0.62%
///            2.0  0.27%      2.5  0.10%      3.0  0.02%
///
/// **Only smeared particles pay it.** A plume's puffs are metres across and
/// were never sub-pixel; a droplet stretched along its own motion is thin in
/// the other axis by construction, which is exactly the primitive the floor
/// exists for.
static const float PARTICLE_MIN_PIXELS = 2.5;

struct ParticleOutput {
    float4 position : SV_Position;
    float2 local : TEXCOORD0;   // -1..1 across the quad
    float4 color : COLOR;
    // Fogged like everything else, so a distant plume recedes instead of
    // hanging in front of the haze at full strength.
    float3 worldPos : WORLDPOS;
    nointerpolation float seed : SEED;
    /// 1.0 for a particle that adds light instead of covering what is behind
    /// it. Carried from the sign of the radius — see `ParticleInstance`.
    nointerpolation float additive : ADDITIVE;
    /// 1.0 when this particle is a flame field rather than a sprite.
    ///
    /// **Carried as the sign of the authored red channel**, which is free: the
    /// schema clamps colour to `[0, 1]`, so the sign bit was unused — exactly
    /// the trick the sign of the radius already plays for `additive`. Being
    /// per-particle rather than per-scene is what lets one scene hold a
    /// campfire and an explosion.
    nointerpolation float flame : FLAME;
    /// xyz the emitter's world origin, w the flame's half-height.
    nointerpolation float4 origin : FLAMEORIGIN;
};

[shader("vertex")]
ParticleOutput particleVertexMain(uint vertexID: SV_VertexID) {
    uint index = vertexID / 6;
    uint corner = vertexID % 6;
    ParticleInstance p = push.particles[index];

    // Two triangles, wound so the quad covers -1..1 on both axes.
    const float2 CORNERS[6] = {
        float2(-1, -1), float2(1, -1), float2(1, 1),
        float2(-1, -1), float2(1, 1), float2(-1, 1),
    };
    float2 local = CORNERS[corner];

    float3 forward = normalize(push.environment[0].eye.xyz - p.position.xyz);
    // Guard the degenerate case: a particle directly above the eye makes the
    // cross product with world up vanish, and the quad collapses to a line.
    float3 reference = abs(forward.y) > 0.99 ? float3(0, 0, 1) : float3(0, 1, 0);
    float3 right = normalize(cross(reference, forward));
    float3 up = cross(forward, right);

    // Rotated per particle so a plume of identical round sprites does not read
    // as a grid. Derived from the instance rather than stored: one fewer field
    // uploaded every frame for every particle.
    float angle = frac(sin(float(index) * 12.9898) * 43758.5453) * 6.2831853;
    float sa = sin(angle), ca = cos(angle);
    float2 spun = float2(local.x * ca - local.y * sa, local.x * sa + local.y * ca);

    float quadHalf = abs(p.position.w);
    // **The soot quad has to contain its hull's projection, and the hull leans.**
    // The marched column reaches `bendMax + R * SMOKE_R_MAX` sideways and R
    // vertically, so a quad of half-extent R clips the downwind flank clean off
    // — which is a hard straight edge in exactly the place the chord fix exists
    // to soften. The wind MUST be read from the same environment buffer the
    // fragment shader reads it from, or the quad and the hull lean differently.
    //
    // The random roll is dropped for smoke: it spins a square that has to stay
    // aligned with a bound computed in world axes, and a marched volume has no
    // sprite texture for it to decorrelate anyway.
    if (p.color.r < 0.0 && p.position.w >= 0.0) {
        float lat = sootBendMax(quadHalf) + quadHalf * SMOKE_R_MAX;
        quadHalf = sqrt(quadHalf * quadHalf + lat * lat) * SMOKE_QUAD_PAD;
        spun = local;
    }

    float3 world = p.position.xyz + (right * spun.x + up * spun.y) * quadHalf;
    float4 color = p.color;

    // -- The shutter smear ---------------------------------------------------
    //
    // **A droplet is not a bead, and the reason it looks like one is that a
    // camera has a shutter and this quad did not.** A crown thrown off an
    // impact is a few hundred spheres travelling at 3-8 m/s; a photograph of
    // one records each as a short streak along its own path, and every judge
    // who has looked at `splash.loom` has called it a string of beads.
    //
    // **This is the rain streak's frame, lifted, not a second one.** Same
    // velocity-relative-to-the-camera, same `cross` for the ribbon's side, same
    // screen-space width floor at the same measured 2.5 px, same conservation
    // of light by dividing the opacity by the widening. There is one particle
    // renderer (ADR 0047) and the rain layer is the one place this frame was
    // already solved; copying its *shape* rather than its code is what keeps
    // them one idea, because rain is additive with a `strength` and a particle
    // is blended with an alpha.
    //
    // **Guarded on the smear being longer than nothing, not just on the
    // shutter.** The streak frame is oriented along the motion while the disc
    // frame carries a per-particle random roll; a particle standing still would
    // otherwise change its orientation for no visible gain, and the rising
    // sheet of a splash column is exactly that particle.
    float shutter = p.velocity.w;
    if (shutter > 0.0) {
        // Metres per `RAIN_TICK`, so the timestep is defined in one place —
        // see `EnvironmentData::eyeStep`. A camera travelling with a droplet
        // sees it stand still, which is what a shutter records.
        float3 relative = p.velocity.xyz - push.environment[0].eyeStep.xyz / RAIN_TICK;
        float speed = length(relative);
        if (speed * shutter > 1e-4) {
            float3 along = relative / speed;
            float3 side = cross(along, push.environment[0].eye.xyz - p.position.xyz);
            float sideLength = length(side);
            // End-on, the cross product vanishes and the ribbon would collapse
            // to a line. Any perpendicular does: the quad covers almost
            // nothing there.
            side = sideLength > 1e-4 ? side / sideLength : right;

            // **The screen-space width floor, at 2.5 px because that is the
            // measured number.** At 1 px a primitive still straddles pixel
            // boundaries and produces MORE isolated salt pixels than no floor
            // at all — 0.75% with none, 0.95% at 1 px, 0.27% at 2 px on the
            // rain measurements this is lifted from. Read off the matrix
            // rather than a field-of-view constant, so it is exact whatever
            // the projection is.
            float widen = 1.0;
            if (PARTICLE_MIN_PIXELS > 0.0) {
                float4x4 viewProj = push.objects[push.objectOffset].mvp;
                float4 clipA = mul(viewProj, float4(p.position.xyz, 1.0));
                float4 clipB = mul(viewProj, float4(p.position.xyz + side * quadHalf, 1.0));
                float2 ndcA = clipA.xy / max(clipA.w, 1e-4);
                float2 ndcB = clipB.xy / max(clipB.w, 1e-4);
                // The offset is `quadHalf`, which is HALF the quad's width, and
                // a unit of NDC is `viewport/2` pixels. The two halves cancel,
                // so this is the full screen width in pixels with no factor —
                // the rain version offsets by a full width and carries the 0.5.
                float pixelsWide =
                    length((ndcB - ndcA) * push.environment[0].viewport.xy);
                if (pixelsWide > 1e-4) {
                    widen = max(1.0, PARTICLE_MIN_PIXELS / pixelsWide);
                }
            }

            // The radius is the head of the streak, the smear is its tail:
            // half a shutter's travel each way about the centre, so a droplet
            // standing still is exactly the disc it was.
            float halfLength = quadHalf + 0.5 * speed * shutter;
            float halfWidth = quadHalf * widen;
            world = p.position.xyz
                  + along * (local.y * halfLength)
                  + side * (local.x * halfWidth);
            // Area went up by `widen`, so opacity comes down by it. Without
            // this a droplet gets *brighter* the further away it is, which is
            // what makes a naive width clamp read as a field of glowing
            // scratches — the rain layer's own lesson.
            color.a /= widen;
        }
    }

    ParticleOutput out;
    // `mvp` of the reserved object slot is the view-projection — see the note
    // on `Push::object_offset` in the Rust mirror.
    out.position = mul(push.objects[push.objectOffset].mvp, float4(world, 1.0));
    out.local = local;
    out.worldPos = world;
    out.seed = float(index);
    out.additive = p.position.w < 0.0 ? 1.0 : 0.0;
    // The sign of red selects the flame field. Free: the schema clamps colour
    // to [0, 1] so the bit was unused, and `abs` puts the colour back.
    out.flame = p.color.r < 0.0 ? 1.0 : 0.0;
    // `color`, not `p.color`: the width floor above pays for its widening in
    // the alpha, and reading the instance again here would spend it and keep it.
    out.color = float4(abs(color.r), color.g, color.b, color.a);
    // The emitter's own position and the flame's half-height, so the field is
    // evaluated in flame-local coordinates and does not swim when the sprite's
    // random roll changes.
    out.origin = float4(p.position.xyz, abs(p.position.w));
    return out;
}

/// A soft puff, generated rather than sampled.
///
/// A texture would be the usual answer and would look better close up. This
/// costs no image, no descriptor and no asset to author, and for smoke — which
/// is a soft blob with torn edges — the difference at plume distance is small.
/// The wispiness comes from breaking up the radial falloff with value noise so
/// the edge is ragged rather than a clean circle, which is the single tell
/// that separates a smoke puff from a fuzzy ball.
// ---------------------------------------------------------------------------
// Fire.
//
// **One quad, and the flame is a VOLUME the fragment marches through.** Not a
// sprite, not a flipbook, not a stack of blobs — and, since this slice, not a
// level set on the quad's plane either.
//
// The reason it is one quad is overdraw. That argument was written against an
// `R8G8B8A8_SRGB` target, where every additive blend clamped at 1.0 with no
// rolloff: measured on the sprite path, red pinned after 2.6 overlapping
// particles against about thirty alive, so the middle of the fire was always
// going to be white however its shapes were authored. **ADR 0018 replaced that
// target with an HDR float frame and a tonemap, so the clipping half of the
// argument is void** — the half that stands is that a single quad is one
// coherent piece of geometry with an analytic silhouette, which is what lets
// the shape be a function rather than a texture.
//
// What the quad now carries is a scalar field in world space, sampled
// `FIRE_STEPS` times along each view ray between the front and back of the
// emitter's own sphere, accumulating emission against transmittance. The
// silhouette machinery underneath — the domain warp, the fold amplitude, the
// threshold above the field's supremum, the fuel lobes — is unchanged and is
// documented constant by constant below. What changed is that it is evaluated
// along a line instead of on a plane, and everything that follows from that is
// documented at `FIRE_STEPS`.
// ---------------------------------------------------------------------------

/// Cycles per metre of the coarsest octave.
static const float FIRE_HZ = 2.2;
/// How much the field is squashed vertically AFTER the warp. A flame's
/// features are far taller than they are wide, and this is what makes them so.
static const float FIRE_SQUASH = 0.30;
/// How fast the field is dragged downward through the flame, in field units a
/// second. The flame rises because the pattern moves down past a fixed
/// envelope — the same trick a waterfall texture plays.
static const float FIRE_RISE = 1.35;
/// Flames accelerate as they rise, so the field is stretched with height and
/// features lengthen toward the tips.
static const float FIRE_ACCEL = 1.9;
/// Domain warp amplitudes, in lattice cells.
///
/// **Set by the fold condition, not by taste.** A 2-octave normalised fBm has
/// a gradient near 0.5 per cell, so `p -> p + a*w(p)` folds where `a` is above
/// about two cells. Below that the silhouette is merely wavy; a tip that hooks
/// back on itself needs the map to fold, and that is the difference between a
/// flame and a flag.
static const float FIRE_WARP_XZ = 2.6;
static const float FIRE_WARP_Y = 3.4;
/// Where the field stops being flame. Rises with height so tongues pinch off
/// rather than all reaching the top together.
///
/// **`FIRE_T1` must EXCEED the field's supremum, and that is arithmetic rather
/// than taste.** `fireFbm` divides by the sum of its amplitudes and
/// `loom_value_noise` is on [0,1), so its supremum is exactly 1.0 and its
/// distribution is narrow — mean 0.5, sigma about 0.15. At the previous 0.66,
/// roughly a tenth of the field still cleared the threshold at the very top of
/// the quad, so a tenth of the column stayed lit for ever: the flame could not
/// terminate at any height, and no amount of adjusting the exponent or the
/// cone could make it, because the ceiling was never above the field's tail.
/// Above 1.0 extinction is certain rather than probabilistic, and because the
/// distribution is narrow the crossing is steep — which is what produces a
/// PINCH that detaches a puff instead of a fade.
///
/// The exponent came down with it: at 2.6 the rise put most of its travel in
/// the top third, so the flame was full width almost to the ceiling and then
/// stopped. At 1.6 the threshold climbs through the field's distribution over
/// the whole height, which is where the necking happens.
static const float FIRE_T0 = 0.30;
static const float FIRE_T1 = 1.05;
static const float FIRE_T_POW = 1.6;
/// How hard the envelope pinches inward with height, and how many lobes the
/// fuel line has. The lobes are what make the tongues SEPARATE at the base
/// rather than being one sheet that noise happens to tear.
///
/// **`FIRE_GAP` is the same arithmetic as `FIRE_T1` and was wrong the same
/// way.** At 0.30 the threshold in the middle of a gap reached only 0.60, and
/// about a fifth of the field clears that — so the gaps between tongues were a
/// fifth lit, every tongue joined its neighbours, and the fire was one
/// connected blob with dark patches rather than separate flames. It has to
/// exceed the field's tail for a gap to be a gap.
static const float FIRE_CONE = 0.55;
static const float FIRE_LOBES = 5.5;
static const float FIRE_GAP = 0.72;

/// **How wide the fire is allowed to be, which is not the same question as how
/// wide the quad is.**
///
/// `cone * 0.22` reaches only 0.31 at the quad's inscribed circle against a
/// threshold base of 0.30, so about a fifth of the field still cleared it out
/// there: a 2.2 m quad standing on a 0.84 m brazier drew a flame two and a half
/// times the width of the thing burning. The march's own sphere ends the
/// integral at the same radius, so this is no longer what stops the quad's edge
/// from being visible — `FIRE_VOL_R` does that, continuously. What it buys is
/// the WIDTH: a fire bounded by a circle it chooses rather than by a square it
/// was handed, which is what turns a row of teeth spanning two and a half bowl
/// widths back into a flame standing in a bowl.
///
/// Same arithmetic as `FIRE_T1` and `FIRE_GAP`: the term takes the threshold
/// past the field's supremum of 1.0 before `radial` reaches the circle, so
/// extinction there is certain rather than probabilistic. The sixth power is
/// what keeps it out of the way inside — it adds 0.07 at `radial` 0.5 and 0.22
/// at 0.6, and only then bites.
///
/// **The soot does not get it, deliberately.** Smoke spreads as it rises and it
/// is the one thing that should be wider than the flame; the marched sphere
/// already ends it with no edge of its own.
static const float FIRE_EDGE_R = 1.0 / 1.15;
static const float FIRE_EDGE_P = 6.0;
static const float FIRE_EDGE_T = 2.0;

/// Where the finest octave is retired. `loom_value_noise` is analytic and has
/// no mip chain, so once that octave falls under a pixel it is a twinkle
/// generator — the same reason as `WATER_DETAIL_RANGE`.
///
/// **The bound is Nyquist in SCREEN space, and a distance in metres was the
/// wrong instrument.** It used to be `1 - length(toEye) / 26`, which retires
/// the octave at a fixed world distance whatever the resolution — so at 4K,
/// where a 2.2 m flame twenty metres away is still 250 pixels wide and its
/// finest features are ELEVEN pixels across, the shader threw them away anyway.
/// That is why the brazier on the deck rendered smooth while the campfire four
/// metres from the camera had structure: not a shading difference, a detail
/// cutoff measuring the wrong thing. It is the same defect class as an
/// anti-aliasing metric taken on a path the filter is not on, which CLAUDE.md
/// records the cost of.
///
/// `fwidth(w)` is metres per pixel on the quad. The finest of three octaves
/// runs at `FIRE_HZ * 2.03^2`, so `metresPerPixel * FIRE_HZ * 4.12` is cycles
/// per pixel and half a cycle per pixel is the sampling limit. Full detail
/// below `FIRE_NYQUIST_LOW`, gone at `FIRE_NYQUIST_HIGH`, faded between so it
/// never pops.
///
/// **This is the screen's bound; the march has its own along the ray** — see
/// `dField` in `flameColor`, where the two are combined with a `min`. They ask
/// different questions (is the octave under a pixel / is it under two steps)
/// and either one alone twinkles.
static const float FIRE_OCTAVE_STEP = 4.12;
static const float FIRE_NYQUIST_LOW = 0.25;
static const float FIRE_NYQUIST_HIGH = 0.50;

/// **The flame is a LINE INTEGRAL now, not a level set on a plane.**
///
/// A view ray through a real flame crosses many small hot pockets and sums
/// them, and that sum is smooth and connected even when every individual
/// iso-surface it crossed is in fragments. The previous shader evaluated one
/// slice of one field at the quad's own depth and thresholded it, so the gaps
/// between the tongues were not dim — they were the background, unattenuated,
/// and no constant inside that formulation could fix it. Every knob that
/// reconnects the shards inside a slice does it by deleting the detail that
/// makes the thing read as fire.
///
/// So the fragment marches the SAME field along the view ray through a sphere
/// of radius `half_h` about the emitter, accumulating emission and
/// transmittance front to back. **Nothing about the silhouette machinery
/// changed**: the domain warp, the fold amplitudes, `FIRE_T1` above the field's
/// supremum, the fuel lobes and the post-warp squash are all evaluated exactly
/// as before, once per sample instead of once per pixel.
///
/// Three things fall out of the integral that had to be built by hand before,
/// and their constants are deleted rather than retuned:
///
/// - **Coverage.** `cover = saturate(e / fwidth(e))` is gone. The silhouette is
///   now where the chord through the level set reaches zero length, and a ray
///   grazing a smooth iso-surface leaves it like a square root — a steeper edge
///   than the analytic one it replaces, with no screen-space derivative in it
///   at all, so the quad-derivative trap the water shoreline is named for no
///   longer applies here.
/// - **The floor** (`FIRE_TAU_FLOOR`). It existed because brightness inside the
///   coverage edge was cubic in depth, so the outermost pixels read black while
///   still being fully opaque — the shader painting the halo it exists to
///   remove. In an integral the outermost pixels emit little AND occlude
///   little, so they are simply not there.
/// - **The continuum** (`FIRE_GLOW*`). A hand-built unthresholded veil filling
///   the gaps is what a line integral produces on its own: a ray through a gap
///   still crosses material at other depths, just less of it, so the gap comes
///   out dim rather than absent. It is the same term, derived instead of
///   authored, and it moves with the camera instead of being pinned to the
///   billboard.
///
/// **`FIRE_STEPS` is set by the field's Nyquist and then by residual banding,
/// and by neither one alone.**
///
/// The chord is `2 * half_h` — 2.2 m in both scenes that author a flame — and
/// the finest octave of the 3-octave field has a period of
/// `1 / (FIRE_HZ * 2.03^2)`, about 0.11 m. Sixteen steps is 1.25 samples per
/// period, under Nyquist, and `dField` below then retires most of that octave;
/// by 32 it survives the march. That is the floor.
///
/// The ceiling is what the trapezoid rule has left over. Re-measured on `deck`
/// at 3840x2160 after the ray dither came out: at 32 the flame's flanks carry
/// faint horizontal striations — 18,399 pixels differ from 48 by more than 8
/// of 255, and they are the stripes — and at 48 there are none. 64 is no better
/// than 48. The cost is paid in flame pixels only and is measured in the commit
/// message.
static const int FIRE_STEPS = 48;
/// The marched volume, as a fraction of `half_h`. **The quad's inscribed
/// sphere, and that is arithmetic rather than taste** — `particleVertexMain`
/// builds the quad as `right * spun.x + up * spun.y` scaled by `half_h` over
/// corners at (±1, ±1), so `half_h` is the largest radius that stays inside the
/// geometry on every side at every billboard orientation. A larger sphere is
/// cut by the quad's own edge, which draws a straight vertical line up both
/// sides of the fire. Fitting inside costs nothing: the chord length falls to
/// zero continuously at the sphere, so the boundary carries no edge of its own.
static const float FIRE_VOL_R = 1.0;
/// Extinction per metre of fully-lit flame. A 0.5 m run of core material at
/// 4.0 covers 86% of what is behind it, which is what makes the fire an object
/// rather than a tint; a near-transparent core was the single largest defect in
/// the slice version and this is the constant that owns it.
static const float FIRE_SIGMA = 4.0;
/// The field width over which a sample goes from empty to full density.
///
/// **Narrow on purpose, and this is the constant that decides between fire and
/// a fireball.** The silhouette's crispness survives the integral only while
/// each sample's density is a steep function of `e`: then the visible edge is
/// the fold where the chord through the iso-surface vanishes, which leaves like
/// a square root and is steeper than a linear ramp. Widen it and the density
/// becomes a smooth blob of the envelope, the chord shortens gently instead of
/// pinching, and the result converges on the fireball this design exists to
/// escape — the same failure a soft `cover` produced, arrived at along the ray
/// instead of across the screen.
static const float FIRE_DENS_W = 0.05;
// **THERE IS NO RAY DITHER, AND THAT IS A MEASURED DECISION — read this before
// adding one back.**
//
// The march used to start each ray at a dithered offset into its first step,
// because with the RECTANGLE rule a ray grazing a nearly-tangent iso-surface
// crosses material thinner than one step, a sample is inside or outside with
// nothing in between, and the contour of that flip is a row of evenly spaced
// spikes off the flame's flanks — a comb. Three ways of paying for it were
// rendered at 3840x2160 on `deck` and on `campfire` and all three are visible:
//
//   - **Interleaved gradient noise.** A regular diagonal halftone across the
//     whole flame body. IGN is designed to be cleaned up by a temporal or a
//     spatial filter and this project has neither, so what is left is a
//     periodic texture the eye locks onto — not grain.
//   - **A hash of the pixel coordinate.** No period to see, and much worse in
//     amplitude: white noise has the variance IGN's low-discrepancy pattern
//     exists to avoid, and the flame came out salt-and-peppered.
//   - **Nothing, under the rectangle rule.** Step quantisation instead.
//
// The trapezoid rule in the loop below removes the cause rather than dithering
// the symptom, and it costs nothing (see its comment). With it, no dither at
// any amplitude is measurably better than none: the comb does not appear on
// either scene at 4K, and the flame body is smooth with no pattern in it.
//
// **What is left is not quadrature and no dither would have hidden it.** The
// interior of a tongue carries nested contour arcs — a rolled-scroll look, most
// visible in motion. They survive the trapezoid, they survive making `fireRamp`
// C1 at its rungs, and widening `FIRE_DENS_W` to 0.14 only thins the flame
// without touching them. They are the domain warp's FOLD seen in depth: the
// warp amplitude is deliberately above the fold condition, a folded map has
// cusps, and a line integral through one crosses the folded sheet several
// times. Every one of the three variants judged in this investigation shows
// them. Removing them means lowering the fold amplitude, which is the one thing
// the silhouette is made of.
/// **Soot: material that occludes and does not emit.**
///
/// The flame's own products, cooled past the point where they radiate. It is
/// the same field under its own, lower ceiling, so it hugs the tongues rather
/// than being a second shape, and it contributes to transmittance with no
/// contribution to `heat` — which against a premultiplied target is exactly a
/// black smoke that eats the background. Gated on height because there is no
/// soot at the fuel line: it is what the fire has already burned.
///
/// **Its own threshold is the whole of it — see the note in `fireField`.**
/// Offsetting `FIRE_T1` instead was tried and produced no smoke above the
/// flame, which is the one place smoke is.
static const float FIRE_SOOT_T1 = 0.70;
static const float FIRE_SOOT_W = 0.30;
static const float FIRE_SOOT_SIGMA = 3.0;
static const float FIRE_SOOT_H = 0.30;
/// How much accumulated hot material it takes to reach the top of the ramp.
///
/// **The ramp is driven by the integral, not by one sample's depth into the
/// level set.** `heat` is the transmittance-weighted quantity of emitting
/// material the eye actually sees along the ray, bounded by one because the
/// weights are a telescoping partition of `1 - T`. So a ray through the core
/// crosses a lot of it and lands on the ramp's white rung, while a ray clipping
/// a tongue's flank crosses little and lands on deep amber. That gradient is
/// what the deleted `FIRE_TAU_DEN` and `FIRE_TAU_EDGE` were approximating from
/// a single sample, and here it is the thing itself.
///
/// Judged on `deck` at 3840x2160: at 1.35 the ramp's white rung is reached by
/// heat 0.75, which is most of the flame's body and reads as a blown-out
/// highlight rather than a core; at 0.90 nothing reaches it and the fire is one
/// flat orange. 1.15 puts white above heat 0.87, which in these two scenes is
/// the core alone.
static const float FIRE_HEAT = 1.15;
/// Cooling with height and at the fuel line. Both are physical and neither
/// falls out of the integral: a flame is dimmer and redder low down because it
/// has not finished mixing, and cooler at the tip because it is running out.
/// The cooling at the FLANKS that used to sit here is gone — a grazing ray now
/// accumulates little heat because it genuinely crosses little material.
static const float FIRE_TAU_H = 0.42;
static const float FIRE_TAU_BASE = 0.45;
static const float FIRE_TAU_BASE_H = 0.18;
/// Peak occlusion, so a flame is never quite a wall.
static const float FIRE_OPACITY = 0.92;

/// How strongly a smoke puff takes the scene's point lights.
///
/// One, meaning the sprite is shaded exactly as an opaque Lambertian sphere of
/// its authored albedo would be. Anything else is a fudge factor asking to be
/// tuned per scene, and the knob that actually belongs to the author is the
/// albedo — which is `color_start`/`color_end`, already in the schema.
static const float SMOKE_LIGHT = 1.0;

// ---------------------------------------------------------------------------
// The soot volume — ADR 0020's line integral, with the emission taken out and
// a plume put in its place.
//
// **The selector is `flame && !additive`, and it costs no schema.** The sign of
// the authored red already says "this quad is a marched field" and the sign of
// the radius already says "this quad emits light". A marched field that does
// not emit is a field that only absorbs, which is what soot is. `plume.loom`,
// `plume_gale.loom` and `plume_roof.loom` are the only scenes that author that
// pair; every other `flame = true` scene is additive, so every blessed additive
// reference is untouched by construction. (This block used to claim no scene
// authored it at all, which stopped being true the day `plume.loom` landed.)
//
// **The marched region is DERIVED from the envelope's support, never the other
// way round.** It used to be an upright ellipsoid while `sootEnvelope` described
// a flaring, wind-bent column: at `plume.loom`'s 3.2 m/s the envelope escaped
// the ellipsoid from h ~ 0.3 up, the pre-noise density at the marched boundary
// ran 0.68-0.93 over h = 0.6-0.8, and ~89% of the authored lean was clipped. The
// march stopped at near-full density, so the silhouette the eye saw was the
// ellipsoid's analytic arc with noise visible inside it — the "balloon with hard
// edges". `columnChord` is that support, bounded in closed form, so a future
// change to the envelope cannot reintroduce the escape.
//
// **One quad per plume, and that is the overdraw answer.** The fire's comment
// block above already made this argument for emission; it is stronger here. A
// sprite plume is ~700 translucent quads with an overdraw of thirty; this is
// one quad with an overdraw of exactly one, and its cost is
// `covered pixels x steps`, which is two constants away rather than a
// population away.
// ---------------------------------------------------------------------------

/// How many samples span the chord. **A quarter of `FIRE_STEPS`, and the
/// reason is Nyquist rather than thrift.** A flame's field is thresholded just
/// under its own supremum, so its density is very nearly a step function of the
/// field and the march is resolving a discontinuity; soot's is a smooth ramp
/// over `SMOKE_W`, which is band-limited and needs far fewer samples to
/// integrate. What it does need is the same trapezoid — see `hotPrev` in
/// `flameColor` for what the rectangle rule draws.
///
/// **Swept, on the two instruments that answer different halves of it.**
/// `plume` at 1920x1080, forward pass, against a 64-step march as the converged
/// reference; flicker at the scene's own camera, held still, 12 ticks apart:
///
///     steps         4      6      8     12     16     20     28     64
///     forward ms  0.518  0.554  0.583  0.660  0.735  0.799  0.940  1.70
///     mean diff   0.661  0.366  0.246  0.155  0.110  0.083  0.052   -
///     picture/ms   -      8.4    4.0    1.22   0.60   0.42   0.22   -
///     flicker       -      -    0.1223 0.1152 0.1129 0.1111 0.1122  -
///
/// Picture-per-millisecond has a knee between 8 and 12 and is flat after it, so
/// a still alone would argue for 12. **Flicker is what argues for 20**, and it
/// is the instrument that matters here: the sample positions slide along the
/// ray as the camera or the field moves, so an under-resolved march does not
/// look soft, it *crawls*. It falls to a floor at 16-20 and 28 does not beat
/// 20 — that last pair is the noise floor, which is how the floor is known to
/// be one. 20 is the first count at it.
///
/// A 12-step build is a legitimate quality setting and this table is where it
/// goes; it is 0.14 ms cheaper and reaches the same still.
static const int SMOKE_STEPS = 20;
/// Stop marching once this little light gets through. A plume is opaque in its
/// middle long before the far side of the column, and the samples past that
/// point are multiplied by a transmittance under 2%.
static const float SMOKE_T_MIN = 0.02;
/// Width of the column, as a fraction of its own half-height. **The quad is
/// square** — `ParticleEmitter.size` is one radius — so a column has to be
/// carved out of it, and everything outside is rejected by the chord test
/// before a single noise lookup.
///
/// **Not an aspect ratio any more.** It scaled the marched ellipsoid's
/// horizontal semi-axes when this was an ellipsoid; now it is one factor in the
/// cone's radius and in `SMOKE_R_MAX`, and nothing has semi-axes.
static const float SMOKE_ASPECT = 0.55;
/// The plume's radius at the source, and how much it flares by the top.
///
/// **0.16 rather than 0.34: the neck is about a fire's width, and the flare is
/// what widens it.** At 0.34 the column left the fuel already 0.75 m across,
/// which is what put a wide flat lid under the plume where a neck belonged.
static const float SMOKE_R0 = 0.16;
static const float SMOKE_FLARE = 0.95;
static const float SMOKE_FLARE_P = 0.75;
/// Height over which the column establishes itself above the source, and the
/// height at which it starts dissipating. Without the first the plume begins as
/// a flat disc sitting on the fire.
///
/// **`SMOKE_FOOT` is a function of world height alone, so it draws a horizontal
/// isoline** — and that isoline is the straight bottom edge the plume used to
/// have. It cannot be removed (a plume with no foot is a disc), so it is made
/// short instead: 0.06 establishes the neck over 0.24R, which at `plume.loom`'s
/// R = 4 is under a metre and sits inside the flame rather than a metre above
/// it. The rest of the cure is that the base is now the flame's position rather
/// than a hand-typed centre, so the ramp happens where the fire is.
static const float SMOKE_FOOT = 0.06;
static const float SMOKE_TOP = 0.80;
/// Widest the cone ever gets, in units of R — `SMOKE_ASPECT * (SMOKE_R0 +
/// SMOKE_FLARE)`, the h = 1 value of the radius profile. Slang has no constexpr
/// call, so it is written out; change it with the three above it.
static const float SMOKE_R_MAX = SMOKE_ASPECT * (SMOKE_R0 + SMOKE_FLARE);
/// How far the true `h^1.6` bend can stray from the LINEAR shear the marched
/// cylinder is built on, in units of `bendMax`.
///
/// **Derived, not tuned: it is `max over h in [0,1] of (h - h^1.6)`, which is
/// 0.1713 at h = 0.457**, rounded up to 0.18. That is the whole containment
/// argument — the sheared cylinder's radius is `R * SMOKE_R_MAX +
/// SMOKE_BEND_SLACK * bendMax`, and every point of the envelope's support is
/// inside it because the radial term is bounded by the first summand and the
/// bend's deviation from the shear by the second.
///
/// **It is a function of the 1.6 in `sootEnvelope`.** Retune that exponent and
/// this number is wrong, containment fails, and the hard edge comes back — as a
/// hard cylinder this time. Recompute them together.
static const float SMOKE_BEND_SLACK = 0.18;
/// How much the quad is grown past the hull's own bounding radius.
///
/// `ponytail: a fixed pad, sound while the hull's radius over its distance to
/// the eye stays under about 0.5 — a perspective quad under-covers a sphere by
/// roughly that ratio. Make it a function of distance if a scene ever has the
/// camera inside a plume.`
static const float SMOKE_QUAD_PAD = 1.15;
/// How far the top of the plume is carried downwind, in plume radii per m/s.
/// **P1's wind, from the environment buffer** — the same `wind` the grass bends
/// to and the foam drifts on, so a scene's smoke and its grass lean together.
static const float SMOKE_DRIFT = 0.26;
/// Metres per second the material rises. This is what makes the pattern a
/// Lagrangian one: a feature is born at the source and travels up, rather than
/// the field sliding through a stationary envelope.
static const float SMOKE_RISE = 0.55;
/// Turns of the column per unit height. Entrainment twists a plume; without it
/// the rising features climb in a perfectly straight line and read as a
/// scrolling texture however good the noise is.
static const float SMOKE_TWIST = 1.30;
/// Cycles of the coarsest octave across the plume's own half-height.
///
/// **The number is set by the plume's RADIUS, not by its height, and that is
/// what 2.6 got wrong.** `SMOKE_ASPECT` makes the column 0.55 as wide as it is
/// tall, so 2.6 cycles per half-height is 1.4 cycles across the whole width —
/// one blob of noise, laid over the envelope. The envelope's radial term is a
/// linear cone with a definite boundary, and with only one blob to erode it
/// **whether the silhouette read as smoke or as an ellipsoid was a lottery on
/// which region of the noise the plume happened to land in.** That is not a
/// tuning complaint: moving the field by `sootOffset` alone turned a wispy
/// column into a hard-edged oval, which is how it was found.
///
/// At 6.5 there are ~3.6 cycles across the width, the noise erodes the cone
/// everywhere, and the silhouette is the field's rather than the envelope's.
/// Rendered at offsets 0, 8 and 32 it reads as the same *kind* of thing three
/// times, which is the property that was missing:
///
///     SMOKE_HZ   2.6                    4.5              6.5
///     offset 0   wispy, hard right edge  billowing        billowing
///     offset 8   solid oval, hard edge   soft, some edge  billowing
///     offset 32  solid oval, hard edge   -                billowing
///
/// It is scale-invariant by construction — the field is sampled at
/// `SMOKE_HZ / R` — so this is a statement about how detailed soot is, not a
/// per-scene tune. It costs nothing: the same three octaves are evaluated, and
/// the step bound below retires the finest to ~0.33 in return.
static const float SMOKE_HZ = 6.5;
/// Amplitude of the divergence-free warp, and the finite-difference step the
/// stream function's perpendicular gradient is taken with.
///
/// **Divergence-free is the whole point and it is why this is not the fire's
/// warp.** `fireField` displaces by two independent noise fields, which has
/// sources and sinks: over a flame's ~0.2 s residence time that is invisible,
/// and over a plume's several seconds the sinks pull density into knots that
/// read as blobs — the "procedural blob rather than a plume" failure. The
/// perpendicular gradient of a scalar stream function has zero divergence in
/// the plane it is taken in, which is the meridional (radial, vertical) plane
/// where a plume's vortex rings actually roll. The third noise axis varies
/// slowly across those planes, so the field is 2.5D: exactly divergence-free
/// where the rolling happens and only slowly varying across it.
static const float SMOKE_WARP = 0.55;
static const float SMOKE_EPS = 0.18;
/// Where the noise has to reach before there is any soot, and over what width
/// it arrives. `SMOKE_T0` below the field's 0.5 mean is what leaves holes for
/// the sky to come through, which is most of what separates a plume from a
/// cone of fog.
static const float SMOKE_T0 = 0.36;
static const float SMOKE_W = 0.30;
/// How much the threshold rises by the top of the column.
///
/// **The top has to break into wisps, not fade as a solid mass.** `SMOKE_TOP`
/// alone scales the whole envelope down together, so the plume dissolves while
/// keeping its outline — a ghost of the same shape. Raising the threshold with
/// height instead means the field's low regions drop out first and the high
/// ones survive, so the column comes apart into separate lumps of soot the way
/// a real plume does. Same trick `FIRE_T1` plays on the flame's tip.
///
/// 0.36 + 0.25 = 0.61 stays under the fbm's supremum, so the top is thinned and
/// not erased; the top fade still does the erasing.
static const float SMOKE_T0_RISE = 0.25;
/// Where the rim erosion starts, as a fraction of the cone's own radius, and
/// how much threshold it has added by the cone itself.
///
/// **The envelope was a pure multiplier, so the silhouette was the envelope's.**
/// `saturate((d - thr) / W) * body` is nonzero wherever `body` is, so the support
/// ended exactly on `sootEnvelope`'s cone. That boundary is faint, but it is
/// analytic: it is a dead-straight line that the noise cannot move, and the outer
/// third of the radius reads as a uniform wash inside it. At `plume_roof`'s
/// camera pulled in to 5.5 m it is a straight edge 300 px long with a haze
/// behind it, which is the reported "hard-edged silhouette".
///
/// **The marched cylinder is NOT that boundary, and this was measured before it
/// was fixed.** Doubling `rc` moves the silhouette under 4 px; scaling `rMax` by
/// 0.75 moves it in by exactly 0.75 (327/336/348 -> 369/372/378 at three rows).
/// `columnChord`'s containment holds; the envelope is what was showing.
///
/// Raising the threshold across a band at the rim makes the support an
/// ISO-CONTOUR of the field instead: soot survives out there only where the noise
/// is high, so the boundary is torn at the fbm's own frequency and moves as the
/// field moves. Inside `SMOKE_RIM` nothing changes at all, which is why the
/// column keeps its body — a rise driven by `1 - body` was tried first and eats
/// the core, because `body` is a linear cone and is already 0.7 a third of the
/// way out. That version left a thin broken wisp.
///
/// **The bound is closed form, which is the point of a band rather than a fade.**
/// `d < 1` by construction — `loom_lattice` is `(hash >> 8) / 2^24` in `[0, 1)`,
/// `loom_value_noise` lerps those corners, and `fireFbm` normalises by its own
/// amplitude sum — and the threshold is at least `SMOKE_T0 + SMOKE_T0_RISE * h`,
/// so density is zero unless
///
///     rim < (1 - SMOKE_T0 - SMOKE_T0_RISE * h) / SMOKE_EDGE
///
/// i.e. the support lies inside
///
///     q < SMOKE_RIM + (1 - SMOKE_RIM) * (1 - SMOKE_T0 - SMOKE_T0_RISE * h)
///                                     / SMOKE_EDGE
///
/// which at 0.55 / 1.0 is **q <= 0.838 at the foot, 0.782 at mid height and
/// 0.726 at the top** — a guaranteed 0.40 m of clear air inside the cone at
/// `plume.loom`'s R = 4 (rMax 2.44), on top of `columnChord`'s own margin. The
/// worst case over 4,000,000 samples of the actual 3-octave `fireFbm` is 0.9472
/// and its 99.9th percentile is 0.8356, so the boundary a frame actually draws
/// sits near **q = 0.76 / 0.71 / 0.65**; the gap between those two rows is where
/// the tearing lives. Lower `SMOKE_EDGE` past `1 - SMOKE_T0 = 0.64` and the
/// guarantee is gone and the straight rim comes back.
static const float SMOKE_RIM = 0.55;
static const float SMOKE_EDGE = 1.0;
/// Extinction per metre at full density.
static const float SMOKE_SIGMA = 3.2;
/// Henyey-Greenstein asymmetry. Positive is forward-scattering, which is what
/// gives a plume a bright rim when the sun is behind it.
static const float SMOKE_G = 0.42;
/// How much sky a sample deep inside the column still sees. Not zero: a real
/// plume's core is dark, not black, and zero here makes it a hole.
static const float SMOKE_AMB_FLOOR = 0.18;


/// Fractal noise for the flame. `detail` fades the finest octave out with
/// distance rather than dropping it abruptly, which would pop.
float fireFbm(float3 p, int octaves, float detail) {
    float sum = 0.0;
    float amp = 0.5;
    float norm = 0.0;
    for (int i = 0; i < octaves; ++i) {
        float w = (i == octaves - 1) ? detail : 1.0;
        sum += loom_value_noise(p) * amp * w;
        norm += amp * w;
        p *= 2.03;
        amp *= 0.5;
    }
    return sum / max(norm, 1e-4);
}

/// Deep amber through to a small white-hot core.
///
/// **The peak used to be capped below 1.0, and ADR 0018 is what removed the
/// reason.** The cap existed because red at 1.0 partway up the ramp reproduced
/// the hue rotation this whole approach exists to avoid: against a fixed-point
/// target, the moment anything else added light — the scene's own `Light`, a
/// second flame behind — red was already at the clip and the sum rotated
/// orange toward yellow toward white. Nothing clips now, and the shoulder
/// scales a triple by one factor, so a value above 1.0 comes back compressed
/// with its hue intact instead of rotated.
///
/// **Only the top two rungs moved, and that is the whole trick.** Scaling the
/// ramp uniformly was tried on paper first and is wrong: at 5x the *body* rung
/// lands at sRGB 252 red against 156 green, which is the bright orange the cap
/// was protecting against, arriving by a different route. The first three
/// rungs are below the knee and come out of the shoulder untouched, so the
/// flame's body is bit-identical and only its tip is re-authored.
///
/// The ladder, computed through the shoulder at exposure 1.0:
///
///     t 0.45  (0.62, 0.21, 0.02)  ->  sRGB (206, 126,  39)   deep orange
///     t 0.70  (1.05, 0.62, 0.14)  ->  sRGB (242, 192,  97)   bright amber
///     t 1.00  (2.60, 1.90, 1.20)  ->  sRGB (252, 219, 179)   warm white
///
/// The reference photograph is deep amber with white over maybe two percent of
/// its area, which is what the last rung's width buys.
float3 fireRamp(float t) {
    t = saturate(t);
    float3 c = lerp(float3(0.00, 0.000, 0.000), float3(0.22, 0.012, 0.001), saturate(t / 0.15));
    c = lerp(c, float3(0.48, 0.090, 0.006), saturate((t - 0.15) / 0.30));
    c = lerp(c, float3(0.62, 0.210, 0.020), saturate((t - 0.45) / 0.25));
    c = lerp(c, float3(1.05, 0.620, 0.140), saturate((t - 0.70) / 0.20));
    c = lerp(c, float3(2.60, 1.900, 1.200), saturate((t - 0.90) / 0.10));
    return c;
}

/// One sample of the flame's field, in emitter-local space.
///
/// **This is the slice shader's whole body, unchanged, hoisted into a function
/// of an arbitrary point.** That is the entire structural change: it used to be
/// called once, at the fragment's own position on the quad, and it is now
/// called `FIRE_STEPS` times along the view ray. Every constant in it — the
/// fold amplitudes, the threshold above the field's supremum, the fuel lobes,
/// the post-warp squash — means exactly what its own comment above says.
struct FireSample {
    /// Signed distance from the level set, in FIELD units. Positive is inside a
    /// tongue.
    float e;
    /// Height through the flame: 0 at the fuel line, 1 at the quad's top.
    float h;
    /// The same distance for the SOOT, which has its own ceiling.
    float es;
};

FireSample fireField(float3 p, float half_h, float t, float dField, float dWarp) {
    FireSample s;
    s.h = saturate((p.y + half_h) / (2.0 * half_h));

    // **The warp is built BEFORE the vertical squash.** Warping afterwards
    // multiplies the warp's vertical amplitude by 1/FIRE_SQUASH — about seven
    // — along the one axis whose coherence is what makes a feature taller than
    // it is wide, and the tongues dissolve into froth.
    float3 qi = float3(p.x, p.y / (1.0 + FIRE_ACCEL * s.h) - FIRE_RISE * t, p.z) * FIRE_HZ;
    float w1 = fireFbm(qi + 17.1, 2, dWarp) - 0.5;
    float w2 = fireFbm(qi + 43.7, 2, dWarp) - 0.5;
    float3 warp = float3(w1 * FIRE_WARP_XZ, w2 * FIRE_WARP_Y, (w1 - w2) * FIRE_WARP_XZ);
    float3 q = qi + warp;
    q.y *= FIRE_SQUASH;
    float d = fireFbm(q, 3, dField);

    // **The envelope and the fuel OFFSET the threshold; they never multiply
    // the field.** A multiply's edge is the mask's own gradient, which is far
    // softer than the field's, and a soft-edged gap between two tongues is
    // precisely what turns a fire back into a fireball.
    float radial = length(float2(p.x, p.z)) / max(half_h * 1.15, 1e-3);
    float cone = radial * (1.0 + FIRE_CONE * s.h);
    // The fuel line: a few lobes across the base, so tongues start apart
    // rather than being one sheet that noise happens to tear.
    //
    // **`cos`, not `sin`, and the phase is the whole of the difference.** `p`
    // is measured from the emitter, so `p.x = p.z = 0` IS the flame's axis —
    // and `sin(0) = 0` put a fuel-line MINIMUM there, which `gap` then turned
    // into `FIRE_GAP * 0.5 = 0.36` of extra threshold on the one column the
    // fire is standing on. The centre of the fire was the one place guaranteed
    // to have no fuel. With peaks `2*pi / 5.5 = 1.14 m` apart the survivors sat
    // at x = ±0.29 and ±0.86 straddling a dead middle, which is the mechanism
    // behind the three detached shards `lanternhead.loom` records in its own
    // comment — **they are the fuel lobes, not noise, and not the alpha.** The
    // integral connects what is there; it cannot invent fuel the threshold
    // forbade. `cos` puts a peak on the axis, which is where a brazier's fuel
    // is.
    float lobes = 0.5 + 0.5 * cos(p.x * FIRE_LOBES + p.z * 0.7);
    float gap = FIRE_GAP * (1.0 - lobes) * (1.0 - s.h * 0.4);

    // How wide the fire is allowed to be, independent of how wide its quad is.
    // See `FIRE_EDGE_T`.
    float edge = FIRE_EDGE_T * pow(saturate(radial / FIRE_EDGE_R), FIRE_EDGE_P);

    s.e = d - (FIRE_T0 + (FIRE_T1 - FIRE_T0) * pow(s.h, FIRE_T_POW)
             + cone * 0.22 + gap + edge);
    // **Soot gets the same field and a lower ceiling, and the ceiling is the
    // whole of it.** `FIRE_T1` is deliberately above the field's supremum, so
    // above the flame's tip `e` is not merely negative, it is unreachable —
    // hanging a smoke term off it produces no smoke anywhere the flame is not
    // already burning, which is the one place smoke is not. `FIRE_SOOT_T1` sits
    // BELOW the supremum, so the material survives past the height at which it
    // stopped radiating, which is what soot is. The fuel lobes are left out for
    // the same reason: smoke fills the gaps between tongues, it does not have
    // gaps of its own.
    //
    // **It does take the radial bound, and leaving it off was measured wrong.**
    // Soot's ceiling is below the supremum precisely so that it survives where
    // the flame does not — including out at the quad's rim, where the flame's
    // own threshold has long since given up. Without the bound the soot filled
    // the marched sphere at its equator and drew a flat grey pancake lying
    // across the campfire's log, two brazier-widths across and at flame height:
    // a plume is a column, and this is what makes it one.
    s.es = d - (FIRE_T0 + (FIRE_SOOT_T1 - FIRE_T0) * pow(s.h, FIRE_T_POW)
             + cone * 0.22 + edge);
    return s;
}

/// The flame at one fragment of the quad: march the field through the quad's
/// depth and integrate emission against transmittance.
float4 flameColor(ParticleOutput in, float3 toEye) {
    // Flame-local coordinates. **The quad is centred on the emitter, so the
    // flame's base is at its BOTTOM edge and not at its middle** — mapping the
    // height from `w.y / half_h` puts the fire in the upper half only, floating
    // clear of whatever it is supposed to be burning. `fireField` owns that
    // mapping now, for every sample rather than for the fragment.
    float3 w0 = in.worldPos - in.origin.xyz;
    float half_h = max(in.origin.w, 1e-3);

    // **Taken here, above the early return, and that placement is the point.**
    // A quad derivative is only defined while every lane of the quad is still
    // running, and the ray/sphere miss below exits non-uniformly. Reading
    // `fwidth` after it is the water-shoreline trap with a `return` in place of
    // a `discard`. Metres per pixel on the quad; what it is for is at
    // `FIRE_OCTAVE_STEP`.
    float cyclesPerPixel = length(fwidth(w0)) * FIRE_HZ * FIRE_OCTAVE_STEP;

    // The chord of the marched sphere, analytically. `w0` lies in the billboard
    // plane and that plane passes through the centre, so `b` is near zero and
    // the two roots are near symmetric — but solving it properly costs three
    // instructions and stays correct under perspective, where the plane is
    // exactly normal to the ray only at the quad's centre.
    float3 dir = -normalize(toEye);
    float R = half_h * FIRE_VOL_R;
    float b = dot(w0, dir);
    float c = dot(w0, w0) - R * R;
    float disc = b * b - c;
    if (disc <= 0.0) {
        return float4(0.0, 0.0, 0.0, 0.0);
    }
    float sq = sqrt(disc);
    float t0 = -b - sq;
    // `FIRE_STEPS` samples span the chord, so there are `FIRE_STEPS - 1`
    // segments between them — the trapezoid's fencepost, and the field is
    // evaluated exactly `FIRE_STEPS` times as the constant says.
    float dt = (2.0 * sq) / float(FIRE_STEPS - 1);

    // **Octave retirement, twice: once for the screen and once for the step.**
    // The screen term is the original bound, re-derived — an analytic noise has
    // no mip chain, so an octave finer than a pixel is a twinkle generator, and
    // `fwidth` on the quad's own local coordinate is metres per pixel, which is
    // the quantity that question is actually about. A distance in metres was
    // the wrong instrument and is what made the deck brazier smooth at 4K; see
    // `FIRE_OCTAVE_STEP`.
    //
    // The step term is the same argument along the ray instead of across the
    // screen: an octave whose period is shorter than twice the step is
    // point-sampled below Nyquist, and marching it produces exactly the twinkle
    // the screen bound exists to prevent, because a small change in ray
    // direction re-picks which crest each sample lands on. Neither bound
    // implies the other, so both are taken and the tighter wins.
    //
    // The two fbm calls get different bounds because their finest octaves are
    // an octave apart — the warp's is `FIRE_HZ * 2.03`, the field's
    // `FIRE_HZ * 2.03^2`. Sharing one number over-retires the warp, and the
    // warp's fold is the silhouette.
    float far = saturate((FIRE_NYQUIST_HIGH - cyclesPerPixel)
                       / (FIRE_NYQUIST_HIGH - FIRE_NYQUIST_LOW));
    float dWarp = min(far, saturate(1.0 / (FIRE_HZ * 2.03 * 2.0 * max(dt, 1e-4))));
    float dField = min(far, saturate(1.0 / (FIRE_HZ * 2.03 * 2.03 * 2.0 * max(dt, 1e-4))));

    float t = push.environment[0].weather.z;

    // Front to back. `heat` is the transmittance-weighted quantity of emitting
    // material the ray sees; it is bounded by one because its weights are a
    // telescoping partition of `1 - T`.
    float heat = 0.0;
    float T = 1.0;
    // **The segment, not the sample, is what gets composited, and the density
    // over it is the average of its two ends.**
    //
    // This is the trapezoid rule against the rectangle rule, and here it is not
    // an accuracy nicety — it is what removes a visible artifact. `FIRE_DENS_W`
    // is deliberately narrow, so `hot` is very nearly a step function of `e`,
    // and with the rectangle rule the whole segment inherits its start point's
    // verdict. The integral then counts *how many samples landed inside*, which
    // is an integer, and its iso-contours are drawn across the flame's body as
    // nested arcs — an onion ring where a tongue's interior should be, crawling
    // as the field moves. Averaging the two ends makes the density continuous
    // along the ray, so the contours have nothing to sit on.
    //
    // **It costs one extra field evaluation, not `FIRE_STEPS` of them** — each
    // sample is the far end of one segment and the near end of the next, so the
    // loop carries its previous verdict forward. That is why the artifact is
    // fixed here rather than by doubling `FIRE_STEPS`, which costs the same
    // again and only halves it.
    FireSample s = fireField(w0 + dir * t0, half_h, t, dField, dWarp);
    float hotPrev = saturate(s.e / FIRE_DENS_W);
    float sootPrev = saturate(s.es / FIRE_SOOT_W) * saturate(s.h / FIRE_SOOT_H);
    float tempPrev = hotPrev * (1.0 - FIRE_TAU_H * s.h)
                   * lerp(FIRE_TAU_BASE, 1.0, saturate(s.h / FIRE_TAU_BASE_H));

    // Rolled, deliberately. Fully unrolling 48 iterations of seven noise
    // lookups each is a shader Slang refuses to build and a compile time the
    // agent's iteration loop cannot afford.
    [MaxIters(FIRE_STEPS)]
    for (int i = 1; i < FIRE_STEPS; ++i) {
        float3 p = w0 + dir * (t0 + float(i) * dt);
        s = fireField(p, half_h, t, dField, dWarp);

        float hot = saturate(s.e / FIRE_DENS_W);
        float soot = saturate(s.es / FIRE_SOOT_W) * saturate(s.h / FIRE_SOOT_H);
        // Local temperature, and **the ramp is deliberately not applied per
        // sample** — a sum of ramp colours mixes hues and comes out muddy. It
        // is applied once, to the integral, which is what keeps its authored
        // ladder from ember through amber to white intact.
        float temp = hot * (1.0 - FIRE_TAU_H * s.h)
                   * lerp(FIRE_TAU_BASE, 1.0, saturate(s.h / FIRE_TAU_BASE_H));

        float sigma = FIRE_SIGMA * 0.5 * (hot + hotPrev)
                    + FIRE_SOOT_SIGMA * 0.5 * (soot + sootPrev);
        // The emitted temperature is weighted by the emitting material only, so
        // a segment that is half soot does not read as half as hot.
        float emit = (hot + hotPrev) > 1e-6
                   ? (temp * hot + tempPrev * hotPrev) / (hot + hotPrev)
                   : 0.0;
        hotPrev = hot;
        sootPrev = soot;
        tempPrev = temp;
        if (sigma <= 0.0) {
            continue;
        }

        float a = 1.0 - exp(-sigma * dt);
        heat += T * a * emit;
        T *= 1.0 - a;
    }

    float3 rgb = fireRamp(saturate(heat * FIRE_HEAT)) * in.color.a;
    float alpha = (1.0 - T) * FIRE_OPACITY * in.color.a;
    return float4(rgb, saturate(alpha));
}


/// How far downwind the top of the column is carried, in metres.
///
/// One function so `columnChord`, the quad and `sootEnvelope` cannot disagree
/// about the lean — a quad sized from a different wind than the hull is drawn
/// with is the exact bug class the chord fix exists to remove.
float sootBendMax(float R) {
    return abs(push.environment[0].wind.z) * SMOKE_DRIFT * R;
}

/// The chord of the marched region: a capped cylinder about the plume's axis,
/// sheared LINEARLY with the wind.
///
/// **The primitive is derived from `sootEnvelope`'s support and nothing else.**
/// The support is `|p.xz - wd * bendMax * h^1.6| <= R * ASPECT * (R0 + FLARE *
/// h^FLARE_P)` for `h` in `[0, 1]`. Shear the space by `wd * bendMax * h` —
/// affine in `(x, y, z)`, unit determinant, and *linear in the ray parameter*,
/// so `t` comes out in world metres exactly as it did for the ellipsoid — and
/// the support becomes `|q.xz| <= R_MAX * R + bendMax * (h - h^1.6)`. Both terms
/// are bounded above independently, which is what makes containment a one-line
/// inequality rather than a tuning exercise:
///
///     rc = R * SMOKE_R_MAX + SMOKE_BEND_SLACK * bendMax
///
/// Checked numerically at R = 4 and wind 0 / 3.2 / 9.0 / 20.0 m/s: the worst
/// `bend deviation + radius` over 2001 samples of `h` is 2.47 against rc 3.04 at
/// `plume`'s wind and 3.26 against 4.13 at `plume_gale`'s. It is a bound, not a
/// fit, so a tighter one would only save marched length.
///
/// The caps at `y = +-R` cost nothing: the foot ramp is zero at `h = 0` and the
/// top fade is zero at `h = 1`, so the density already vanishes on both planes.
bool columnChord(float3 o, float3 d, float R, out float t0, out float t1) {
    t0 = 0.0;
    t1 = 0.0;
    float bendMax = sootBendMax(R);
    float2 wd = normalize(push.environment[0].wind.xy + float2(1e-4, 0.0));
    float2 oc = o.xz - wd * (bendMax * (o.y + R) / (2.0 * R));
    float2 dc = d.xz - wd * (bendMax * d.y / (2.0 * R));
    float rc = R * SMOKE_R_MAX + SMOKE_BEND_SLACK * bendMax;

    // The caps first, because they bound the ray even when it never leaves the
    // cylinder (looking straight up the column).
    float cap0 = -1e9;
    float cap1 = 1e9;
    if (abs(d.y) < 1e-6) {
        if (abs(o.y) > R) {
            return false;
        }
    } else {
        float ta = (-R - o.y) / d.y;
        float tb = (R - o.y) / d.y;
        cap0 = min(ta, tb);
        cap1 = max(ta, tb);
    }

    float A = dot(dc, dc);
    float cyl0 = -1e9;
    float cyl1 = 1e9;
    if (A < 1e-9) {
        // Along the (sheared) axis: in or out for the whole ray.
        if (dot(oc, oc) > rc * rc) {
            return false;
        }
    } else {
        float B = dot(oc, dc);
        float C = dot(oc, oc) - rc * rc;
        float disc = B * B - A * C;
        if (disc <= 0.0) {
            return false;
        }
        float sq = sqrt(disc);
        cyl0 = (-B - sq) / A;
        cyl1 = (-B + sq) / A;
    }

    t0 = max(cap0, cyl0);
    t1 = min(cap1, cyl1);
    return t1 > t0;
}

/// The plume's envelope: how much of the column is at `p`, before any noise.
///
/// One function so the density and the sun-shadow chord cannot disagree about
/// where the plume is, which is the same reason `fireField` computes soot and
/// flame from one `d`.
float sootEnvelope(float3 p, float R, out float h, out float2 rel, out float q) {
    h = saturate((p.y + R) / (2.0 * R));
    float2 wd = normalize(push.environment[0].wind.xy + float2(1e-4, 0.0));
    // The bend is superlinear in height because drag on a rising parcel has had
    // longer to act on it; a linear lean is a tilted cylinder, not a plume.
    // **The 1.6 here is what `SMOKE_BEND_SLACK` is derived from.** Changing it
    // without recomputing `max(h - h^1.6)` breaks `columnChord`'s containment.
    rel = p.xz - wd * (sootBendMax(R) * pow(h, 1.6));
    float rMax = R * SMOKE_ASPECT * (SMOKE_R0 + SMOKE_FLARE * pow(h, SMOKE_FLARE_P));
    // The radius as a fraction of the cone's own — what `SMOKE_RIM` bands and
    // the only place the cone's width is known, so nothing recomputes it.
    q = length(rel) / max(rMax, 1e-4);
    return saturate(1.0 - q)
         * smoothstep(0.0, SMOKE_FOOT, h)
         * (1.0 - smoothstep(SMOKE_TOP, 1.0, h));
}

/// Where in the noise this particular plume reads its field from.
///
/// **Two plumes in one scene are otherwise the same column twice.** Everything
/// in `sootDensity` is a function of plume-local position and time, so a second
/// emitter marches a bit-identical field and the pair reads as a copy-paste —
/// which is precisely the tell a procedural volume exists to avoid.
///
/// **Bounded, and that is the whole design of it.** `in.seed` is
/// `float(index)`, the particle's slot, which reaches 16,200 on `emberfall`;
/// `seed * scale` at that magnitude lands the lattice coordinate where a
/// `float` has no fractional bits left to distinguish neighbouring samples, and
/// the field degenerates. `frac` of an irrational multiple is equidistributed
/// over `[0, 1)` and never leaves it, so the offset is a well-conditioned
/// lattice coordinate however many particles the emitter has. Three different
/// irrationals rather than one, so the offset is a point in the volume and not
/// a slide along its diagonal.
///
/// The same trap is live one function family over: `in.seed * 7.31` at
/// `particleFragmentMain`'s flipbook is the unbounded form, and is only safe
/// because the sprite path's populations have stayed small.
float3 sootOffset(float seed) {
    return frac(seed * float3(0.6180339887, 0.7548776662, 0.5698402910)) * 32.0;
}

/// Soot density at `p`, in plume-local coordinates.
///
/// `warp` is off for the sun-shadow taps: the shadow integrates a length of
/// column, and the curl's job is the silhouette, which two taps along a chord
/// cannot resolve anyway. That is two fbm evaluations saved per shadow sample.
float sootDensity(float3 p, float R, float t, float3 off, float dF, float dW, bool warp) {
    float h;
    float2 rel;
    float q;
    float body = sootEnvelope(p, R, h, rel, q);
    if (body <= 0.0) {
        return 0.0;
    }
    // **The material coordinate, and it is what makes this rise rather than
    // scroll.** Subtracting `SMOKE_RISE * t` from the vertical axis means a
    // feature keeps its coordinate as it climbs: it is born at the source and
    // carried up, deforming, instead of the envelope standing still while a
    // pattern slides through it.
    float3 m = float3(rel.x, p.y - SMOKE_RISE * t, rel.y) * (SMOKE_HZ / R);
    float a = h * SMOKE_TWIST;
    float ca = cos(a), sa = sin(a);
    m = float3(m.x * ca - m.z * sa, m.y, m.x * sa + m.z * ca);

    // **`off` is added at the lookup, never to `m` itself.** It has to miss two
    // things. The twist rotates `m` about the column's axis, so an offset
    // carried through it would be spun by an angle that grows with height —
    // a second, enormous twist. And the warp takes its radial direction from
    // `m.xz`, so an offset of tens of units there would swamp the radius and
    // leave `rd` nearly constant, which is exactly the meridional-plane
    // property the stream function is built on. Shifting only where the noise
    // is *read* decorrelates the pattern and leaves every geometric quantity
    // in this function untouched.
    if (warp) {
        // Perpendicular gradient of one stream function, in the meridional
        // plane. Three taps, one of them shared with nothing else — see
        // `SMOKE_WARP` for why this is not `fireField`'s two-field warp.
        float rr = max(length(m.xz), 1e-3);
        float3 rd = float3(m.x / rr, 0.0, m.z / rr);
        float psi0 = fireFbm(m + off, 2, dW);
        float psiR = fireFbm(m + off + rd * SMOKE_EPS, 2, dW);
        float psiY = fireFbm(m + off + float3(0.0, SMOKE_EPS, 0.0), 2, dW);
        float dpdr = (psiR - psi0) / SMOKE_EPS;
        float dpdy = (psiY - psi0) / SMOKE_EPS;
        m += (rd * dpdy - float3(0.0, 1.0, 0.0) * dpdr) * SMOKE_WARP * (0.25 + h);
    }
    float d = fireFbm(m + off, 3, dF);
    // **The envelope raises the threshold as well as scaling the result.** The
    // scale alone put the support on the envelope's cone; see `SMOKE_EDGE` for
    // the measurement and for the closed-form radius the support is bounded by.
    float rim = saturate((q - SMOKE_RIM) / (1.0 - SMOKE_RIM));
    float thr = SMOKE_T0 + SMOKE_T0_RISE * h + SMOKE_EDGE * rim;
    return saturate((d - thr) / SMOKE_W) * body;
}

/// Henyey-Greenstein, scaled so isotropic is 1 rather than 1/4pi. Keeping the
/// normalisation out of it means `g` can be tuned without re-tuning albedo.
float smokePhase(float c) {
    float g2 = SMOKE_G * SMOKE_G;
    return (1.0 - g2) / pow(max(1.0 + g2 - 2.0 * SMOKE_G * c, 1e-4), 1.5);
}

/// A soot volume at one fragment: march the plume and integrate scattered light
/// against transmittance.
///
/// `entry` comes back as the world point where the ray enters the hull, which
/// is what the fragment writes as its depth — see `particleFragmentMain`.
float4 smokeColor(ParticleOutput in, float3 toEye, out float3 entry) {
    float3 w0 = in.worldPos - in.origin.xyz;
    float R = max(in.origin.w, 1e-3);
    // Same placement as `flameColor`'s, and for the same reason: a derivative
    // is only defined while every lane is live, and the chord test below exits
    // non-uniformly.
    float cyclesPerPixel = length(fwidth(w0)) * (SMOKE_HZ / R) * FIRE_OCTAVE_STEP;

    // **The chord of the envelope's own support** — see `columnChord`. This used
    // to be an upright ellipsoid, and the mismatch with the flaring, leaning
    // column was the hard silhouette.
    float3 dir = -normalize(toEye);
    float t0, t1;
    entry = in.worldPos;
    if (!columnChord(w0, dir, R, t0, t1)) {
        return float4(0.0, 0.0, 0.0, 0.0);
    }
    entry = in.worldPos + dir * t0;
    float dt = (t1 - t0) / float(SMOKE_STEPS - 1);

    // Octave retirement, both bounds, exactly as `flameColor` derives them.
    float far = saturate((FIRE_NYQUIST_HIGH - cyclesPerPixel)
                       / (FIRE_NYQUIST_HIGH - FIRE_NYQUIST_LOW));
    // **Two octaves for the warp, three for the field, so two different step
    // bounds** — the same split `flameColor` makes and for the same reason. The
    // field's finest octave is one octave above the warp's, so sharing a number
    // either over-retires the warp (and the warp's fold is the silhouette) or
    // under-retires the field (and an octave finer than twice the step is the
    // twinkle generator both bounds exist to prevent).
    float hz = SMOKE_HZ / R;
    float dW = min(far, saturate(1.0 / (hz * 2.03 * 2.0 * max(dt, 1e-4))));
    float dF = min(far, saturate(1.0 / (hz * 2.03 * 2.03 * 2.0 * max(dt, 1e-4))));
    // **The step bound is what a close-up plume runs into**, and it is worth
    // knowing which way round it goes. Walking the camera in makes `far` go to
    // 1 (there are more pixels per metre, so the screen stops being the binding
    // constraint) while `dt` grows with the chord — so a plume filling the
    // frame is *softer* than a distant one, not sharper. That is the bound
    // working: the alternative is a march that crawls. It is also the ceiling
    // on this shape, and the lever is `SMOKE_STEPS`.
    // `ponytail: fixed step count, make it a function of the chord if a scene
    // ever needs a plume the camera walks into.`

    float t = push.environment[0].weather.z;
    float3 off = sootOffset(in.seed);
    float3 sunDir = sunDirection();
    float phase = smokePhase(dot(dir, sunDir));
    float3 ambient = hemisphereAmbient(float3(0.0, 1.0, 0.0)) * ambientStrength();
    float3 sun = sunColor() * sunStrength();

    float3 acc = float3(0.0, 0.0, 0.0);
    float T = 1.0;
    float densPrev = sootDensity(w0 + dir * t0, R, t, off, dF, dW, true);

    [MaxIters(SMOKE_STEPS)]
    for (int i = 1; i < SMOKE_STEPS; ++i) {
        float3 p = w0 + dir * (t0 + float(i) * dt);
        float dens = sootDensity(p, R, t, off, dF, dW, true);
        float sigma = SMOKE_SIGMA * 0.5 * (dens + densPrev) * in.color.a;
        densPrev = dens;
        if (sigma <= 1e-5) {
            continue;
        }

        // **The self-shadow, and it is the whole reason this is a volume.** Two
        // taps along the analytic chord from this sample to where the sun
        // leaves the column. The chord is closed form, so the expensive part is
        // two density evaluations rather than a second march — and the envelope
        // is a good model of the column that does the occluding, which is what
        // makes two taps enough. `ponytail: two fixed taps, raise to a short
        // march if a deep plume's core reads flat.`
        //
        // **`columnChord` again, not a second shape.** Same reason `sootEnvelope`
        // is one function: a shadow chord that leaves a different volume than
        // the march darkens material that is not there.
        float tauSun = 0.0;
        {
            float s0, sExit;
            if (columnChord(p, sunDir, R, s0, sExit) && sExit > 0.0) {
                float s1 = sootDensity(p + sunDir * (sExit * 0.22), R, t, off, dF, dW, false);
                float s2 = sootDensity(p + sunDir * (sExit * 0.58), R, t, off, dF, dW, false);
                tauSun = (s1 + s2) * 0.5 * sExit * in.color.a;
            }
        }

        float hDummy;
        float2 relDummy;
        float qDummy;
        float body = sootEnvelope(p, R, hDummy, relDummy, qDummy);
        float3 light = sun * exp(-SMOKE_SIGMA * tauSun) * phase
                     + ambient * lerp(1.0, SMOKE_AMB_FLOOR, body)
                     + pointLights(in.worldPos + (p - w0), normalize(p + float3(0.0, R, 0.0)),
                                   max(R * 0.25, 0.05));

        float a = 1.0 - exp(-sigma * dt);
        acc += T * a * in.color.rgb * light;
        T *= 1.0 - a;
        if (T < SMOKE_T_MIN) {
            break;
        }
    }

    return float4(acc, saturate(1.0 - T));
}

/// **A volume must not be depth-tested at a plane.**
///
/// The particle pipeline tests LESS against the billboard's own plane, which is
/// the right question for a sprite and the wrong one for a marched hull: the
/// plane runs through the hull's CENTRE, so any geometry nearer than that plane
/// discards every fragment of the quad it covers — the smoke is cut dead
/// straight along the geometry's silhouette with no haze in front of it at all,
/// even where a metre of soot sits between the eye and the wall. `plume_roof`
/// is that frame.
///
/// It was latent while a plume was a centre typed high in the air; anchoring the
/// base at the flame puts the hull down among fuel, walls and ground, which is
/// exactly where the plane cut fires.
///
/// **The smoke branch writes the depth of the hull's ENTRY point instead**, so
/// the test asks "is this geometry in front of the smoke" rather than "in front
/// of the smoke's midpoint". Every other branch writes `in.position.z`, the
/// interpolated value the hardware would have used, so flames and sprites are
/// bit-identical.
///
/// `SV_DepthLessEqual` rather than plain `SV_Depth`: the projection is
/// `perspective_rh` with LESS, so nearer is smaller, and the entry point is
/// never behind the plane — which lets the hardware keep conservative early-Z.
/// The `min` is what makes that promise true rather than merely likely: at the
/// corners of the grown quad a ray can enter the leaning hull *past* the plane,
/// and a fragment that wrote a farther depth under a LessEqual mode is
/// undefined, not just wrong.
///
/// **Known trade, and it is the honest one:** geometry crossing the hull
/// mid-body now over-draws — the full chord is integrated in front of it —
/// instead of hard-cutting. Clamping `t1` per ray to the scene's depth is the
/// correct fix and is blocked here: this pass draws before the depth image it
/// would have to sample. Deferred with the trigger "a wall that must cut a
/// plume mid-body".
[shader("fragment")]
float4 particleFragmentMain(ParticleOutput in, out float depth : SV_DepthLessEqual) : SV_Target {
    depth = in.position.z;
    float3 eyeVec = push.environment[0].eye.xyz - in.worldPos;
    if (in.flame > 0.5 && in.additive <= 0.5) {
        float3 entry;
        float4 smoke = smokeColor(in, eyeVec, entry);
        if (smoke.a <= 0.002) {
            discard;
        }
        // Same view-projection the vertex path used — `mvp` of the reserved
        // object slot.
        float4 clip = mul(push.objects[push.objectOffset].mvp, float4(entry, 1.0));
        if (clip.w > 1e-6) {
            depth = min(clip.z / clip.w, in.position.z);
        }
        float3 fogS = fogAmount(push.environment[0].eye.xyz, in.worldPos);
        // Premultiplied throughout, so the fog target is the fog colour times
        // this fragment's own coverage — blending toward the bare fog colour
        // would add light the fragment does not cover.
        return float4(lerp(smoke.rgb, fogColor(-normalize(eyeVec)) * smoke.a, fogS),
                      smoke.a);
    }
    if (in.flame > 0.5) {
        float4 fire = flameColor(in, eyeVec);
        // **The extinction test has to include the alpha now.** It was an
        // RGB-only sum, which was the right predicate exactly while the alpha
        // was hard-coded to zero: a fragment emitting no light contributed
        // nothing. A fragment that emits no light but still occludes does
        // contribute, and discarding it would put a visible edge along
        // whatever contour the RGB sum happens to cross.
        if (fire.r + fire.g + fire.b <= 0.0008 && fire.a <= 0.0008) {
            discard;
        }
        float3 fogF = fogAmount(push.environment[0].eye.xyz, in.worldPos);
        // **The second site that hard-coded `0.0`, and the alpha has to be
        // fogged along with the colour rather than merely passed through.**
        // Fog sits between the eye and the flame AND between the eye and
        // whatever is behind it, and the background already had its own fog
        // applied when it was drawn — so at `fogF = 1` the right answer is
        // "leave the frame alone", which is alpha 0, not an opaque hole. A
        // flame that kept its opacity while its emission was attenuated would
        // fade to a black silhouette at range, which is worse than the
        // see-through this change exists to fix.
        //
        // **Alpha takes the least-fogged channel**, because alpha is one
        // number and the fog is now three. Above water all three are equal and
        // this is the value it always was; underwater, a flame whose red has
        // been absorbed is still emitting blue and still occludes, so the
        // channel that survives longest is the one that decides whether the
        // fragment is there at all.
        return float4(fire.rgb * (1.0 - fogF), fire.a * (1.0 - min(fogF.r, min(fogF.g, fogF.b))));
    }

    float r = length(in.local);
    // The radial cut belongs to the procedural sprite. A flipbook frame fills
    // its cell, so cutting a circle out of it clips the flame tongues at the
    // corners — which is exactly where they lick.
    bool flipbook = in.additive > 0.5 && push.environment[0].fireFlipbook != NO_TEXTURE;
    if (r > 1.0 && !flipbook) {
        discard;
    }

    float3 tint = float3(1.0, 1.0, 1.0);
    float alpha;

    // **A flipbook, for additive particles, when the scene supplies one.**
    //
    // The procedural falloff below is a blob eaten by noise. That reads as
    // smoke, which is what it was written for, and it cannot read as flame:
    // fire is made of *tongues* — narrow at the base, torn at the edges,
    // licking upward — and a radially symmetric sprite has no way to be one.
    //
    // **The frame comes from the clock and the particle's own seed, not from
    // its age.** No age is carried to the fragment shader, and adding one
    // would mean a new varying on every particle in every scene for a feature
    // only fire uses. Seeding the phase per particle is also what stops sixty
    // sprites playing the same frame together, which is the thing that makes a
    // flipbook read as a flipbook.
    if (flipbook) {
        float2 cell = in.local * 0.5 + 0.5;
        // Discard outside the quad happened above on a radius; a flipbook
        // wants the whole square, so the corners are back in play and the
        // sprite's own alpha is what shapes it.
        float phase = push.environment[0].weather.z * FIRE_FLIPBOOK_FPS
                    + in.seed * float(FIRE_FLIPBOOK_FRAMES);
        uint frame = uint(phase) % uint(FIRE_FLIPBOOK_FRAMES);
        uint gx = frame % uint(FIRE_FLIPBOOK_GRID);
        uint gy = frame / uint(FIRE_FLIPBOOK_GRID);
        float inv = 1.0 / float(FIRE_FLIPBOOK_GRID);
        // Inset by half a texel of the CELL, not of the sheet: without it the
        // linear sampler reaches into the neighbouring frame and every sprite
        // carries a sliver of the next flame along its edge.
        float2 uv = (float2(float(gx), float(gy)) + saturate(cell) * 0.98 + 0.01) * inv;
        float4 book = sampleMap(push.environment[0].fireFlipbook, uv);
        tint = book.rgb;
        alpha = in.color.a * book.a;
    } else {
        // Two octaves of value noise in the quad's own space, offset per
        // particle so neighbours do not share a pattern.
        float2 q = in.local * 1.7 + in.seed * 7.31;
        float wisp = 0.62 * valueNoise(q) + 0.38 * valueNoise(q * 2.4 + 3.7);

        // Falloff from the centre, eaten into by the noise. `smoothstep`
        // rather than a linear ramp: a linear edge on a translucent sprite
        // reads as a visible disc boundary once several overlap.
        float falloff = smoothstep(1.0, 0.15, r);
        alpha = in.color.a * falloff * (0.45 + 0.85 * wisp);
    }
    if (alpha <= 0.002) {
        discard;
    }
    float3 toEye = push.environment[0].eye.xyz - in.worldPos;
    float3 fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    alpha = saturate(alpha);

    if (in.additive > 0.5) {
        // Fog ATTENUATES an emitter, it does not tint it. Blending toward the
        // fog colour would ADD grey light to the scene, so a distant fireball
        // would glow brighter through thicker haze.
        float3 rgb = in.color.rgb * (1.0 - fog);
        // Premultiplied by alpha for the brightness, then alpha zeroed: the
        // blend adds this colour and occludes nothing behind it.
        return float4(rgb * alpha, 0.0);
    }

    // **Smoke is a sphere, and until now it was a disc.**
    //
    // Every alpha-blended particle emitted one flat colour over its whole quad,
    // so a plume was N discs of identical grey stacked up: the opacity
    // accumulated correctly — `1 - (1 - a)^N` — but nothing about the COLOUR
    // said there was more than one, and a stack of same-coloured discs is a
    // stack of discs however soft its edges are.
    //
    // The normal is the hemisphere facing the eye, taken straight off the
    // sprite's own geometry: `origin` is the puff's centre and `origin.w` its
    // radius, so `off + toEye * sqrt(1 - r^2) * radius` points out of the
    // sphere the disc is standing in for. No basis to reconstruct, no varying
    // to add, and it is correct at every billboard orientation because the
    // billboard IS oriented to the eye.
    //
    // **It is ADDED to the authored colour rather than replacing it, and that
    // is what keeps the blast radius honest.** `pointLights` returns zero in a
    // scene with no point lights, so `1 + lit * SMOKE_LIGHT` is exactly 1 and
    // every plume in the repo not standing next to a light renders
    // byte-identical. Where there IS a fire, the near side of each puff picks up
    // its colour and the far side keeps whatever the scene authored — so
    // "genuinely black on the shadow side" becomes a scene rule the engine makes
    // visible, rather than a grey the engine imposes.
    //
    // **What this cannot do, plainly.** Each puff is shaded as if it were alone.
    // Nothing here knows how much smoke lies between this puff and the light, so
    // a deep column does not darken its own middle and the tenth puff in from
    // the edge is lit as brightly as the first. Real smoke is black inside
    // because of that self-shadowing and this is not it; getting it needs the
    // plume's optical depth rendered into an offscreen R16F target and a second
    // draw shaded from it — one full-resolution target, a second pass over the
    // whole particle set, and its own render-graph barriers. What is bought here
    // is the per-puff gradient, which is the difference between discs and volume
    // at plume distance; the column's interior still goes dark, but by
    // transmittance rather than by shadow.
    float3 lightDir = normalize(toEye);
    float3 sphereN = normalize(in.worldPos - in.origin.xyz
                             + lightDir * (sqrt(saturate(1.0 - r * r)) * in.origin.w));
    float3 lit = pointLights(in.worldPos, sphereN, max(in.origin.w, 0.01));
    float3 rgb = in.color.rgb * (1.0 + lit * SMOKE_LIGHT);
    rgb = lerp(rgb, fogColor(-normalize(toEye)), fog);
    // Premultiplied: the pipeline's source factor is ONE.
    return float4(rgb * alpha, alpha);
}

// ---------------------------------------------------------------------------
// The background, as a fullscreen triangle.
//
// No vertex buffer, no mesh, no second pass — three vertices generated from
// `SV_VertexID`, drawn before the scene with depth writes off. A sky that
// needed its own render pass would be the obsolete style CLAUDE.md warns
// about for something this small.
// ---------------------------------------------------------------------------

struct SkyOutput {
    float4 position : SV_Position;
    float2 ndc : TEXCOORD0;
};

[shader("vertex")]
SkyOutput skyVertexMain(uint vertexID: SV_VertexID) {
    // The standard fullscreen triangle: (-1,-1), (3,-1), (-1,3). One triangle
    // rather than two, so there is no diagonal seam to get wrong.
    float2 corner = float2(float((vertexID << 1) & 2), float(vertexID & 2));
    SkyOutput out;
    out.ndc = corner * 2.0 - 1.0;
    out.position = float4(out.ndc, 1.0, 1.0);
    return out;
}

[shader("fragment")]
float4 skyFragmentMain(SkyOutput in) : SV_Target {
    // Unproject both ends of the pixel's ray and take the direction between
    // them. Using only the far plane would be wrong under a perspective
    // divide that is not centred on the eye.
    float4 near = mul(push.invViewProj, float4(in.ndc, 0.0, 1.0));
    float4 far = mul(push.invViewProj, float4(in.ndc, 1.0, 1.0));
    float3 dir = normalize(far.xyz / far.w - near.xyz / near.w);

    // **Submerged, the background is water, not sky.** Looking sideways with
    // no geometry in view, this pass used to paint a horizon and a sun glow
    // from the bottom of a lake.
    //
    // **The direction is computed FIRST now, and the comment that used to sit
    // here was the bug.** It said "there is no direction to compute: the
    // medium is uniform" and returned before the unprojection — which is true
    // of absorption and false of everything else. All the light down here
    // arrived through the surface, so it comes from above, and returning one
    // colour for every direction is a flat teal void with a hard line where
    // the seabed starts. That is precisely what it looked like.
    if (eyeUnderwater()) {
        return float4(underwaterColor(dir), 1.0);
    }

    return float4(skyColor(dir), 1.0);
}

// ---------------------------------------------------------------------------
// Grass
//
// A blade is a cubic Bézier expanded here from `SV_VertexID` alone — no vertex
// buffer, no index buffer, 42 vertices per blade. The CPU uploads what a blade
// *is*; where its triangles are is decided per frame, which is what lets the
// wind bend it.
//
// **The wind bends the control points, it does not translate the blade.** A
// blade whose base moves is grass sliding across the ground — the "swimming"
// artifact — and it is one of the motion-only traps that a still frame cannot
// show. The base is pinned; the tip travels.
// ---------------------------------------------------------------------------

// Segments along a blade. Seven is fifteen vertices plus the tip, the
// published Ghost of Tsushima number, and six vertices per segment because
// this is a triangle list rather than a strip.
static const uint GRASS_SEGMENTS = 7;
static const uint GRASS_VERTS = GRASS_SEGMENTS * 6;

struct GrassOutput {
    float4 clip : SV_Position;
    float3 worldPos : TEXCOORD0;
    float3 normal : TEXCOORD1;
    // x is across the blade, y runs base to tip.
    float2 uv : TEXCOORD2;
    // The blade's albedo: its clump's colour times its own shade, unpacked
    // here rather than in the fragment stage — see `unpackRGB`.
    float3 albedo : TEXCOORD3;
};

/// The clump albedo `loom_cli::pack_rgb` wrote into `shape.w`.
///
/// **Called from the vertex shader only.** The packed value is up to 2^24,
/// which an `f32` holds exactly — but an *interpolated* one is only exact if
/// the barycentrics sum to exactly one, and one ULP at that magnitude is a
/// channel that wrapped. Unpack once, interpolate the colour.
float3 unpackRGB(float packed) {
    float r = floor(packed / 65536.0);
    float g = floor(fmod(packed, 65536.0) / 256.0);
    float b = fmod(packed, 256.0);
    return float3(r, g, b) * (1.0 / 255.0);
}

/// The wind at a point, from the environment block.
/// **Fields, not a struct copy.** `EnvironmentData env = *push.environment`
/// was a whole-struct load, and the struct now ends with sixteen waves — 400
/// bytes nobody here reads, per vertex, if the compiler declines to prove it
/// dead. Reading the four vectors this actually wants costs what it costs.
float3 grassWind(float3 p) {
    float4 wind = push.environment[0].wind;
    float4 weather = push.environment[0].weather;
    WindParams params;
    params.dir_x = wind.x;
    params.dir_z = wind.y;
    params.speed = wind.z;
    params.gustiness = wind.w;
    params.turbulence = weather.x;
    params.ground_drag = weather.y;
    return wind_at(p, weather.z, params);
}

/// A stable per-blade draw for the distance cull.
///
/// **Stable is the whole requirement.** If which blades survive changed frame
/// to frame the field would boil; if it came from the clump, whole tussocks
/// would vanish at once and read as bald patches. So it is hashed from the
/// blade's own position, which never changes.
float grassCullDraw(GrassBlade blade) {
    uint3 q = asuint(blade.position.xyz * 91.7);
    return float(loom_hash(loom_hash(q.x) + loom_hash(q.z) + q.y) >> 8) * (1.0 / 16777216.0);
}

/// The pixel floor below which a blade is widened. **Zero disables it, and it
/// is zero — this is the third time this trick has been measured and the third
/// time it has failed to earn its place.** Do not run the experiment a fourth
/// time without reading this.
///
/// The first two rounds were measured on a broken instrument: `shimmer` framed
/// whole-scene bounds and photographed a bare slab of soil with every blade
/// already culled. Those numbers are void. This table is `cargo xtask shimmer`
/// on the fixed instrument — camera at the scene's authored eye, simulation
/// advancing, 4x MSAA, 640x400 — and the run is deterministic, so a repeat of
/// the same row reproduces to the last digit and every difference below is
/// signal rather than noise:
///
///     floor    meadow   grass_slope
///     off       2.712     1.545      <- shipped
///     0.8 px    2.700     1.569
///     1.0 px    2.688     1.563
///     1.4 px    2.665     1.562
///     2.0 px    2.631     1.554
///     3.0 px    2.583     1.548
///     4.0 px    2.518     1.554
///
/// **It helps `meadow` monotonically and hurts `grass_slope` at every floor.**
/// A tool that improves one grass scene by 0.9% and makes the other worse is
/// not a tool. And the two are not symmetric by accident: `meadow`'s camera
/// sits in the field, so a 0.02 m blade spans 8.98/d pixels here and the floor
/// only bites past 9 m at 1 px; `grass_slope` looks down a 15–40 m slope where
/// almost every blade is sub-pixel, so the floor widens the whole field — and
/// that is the case it loses.
///
/// **The wins that are large enough to see are uniform widening in disguise.**
/// Widening every blade in the scene file gives 0.035 m -> 2.635 and
/// 0.060 m -> 2.338 on `meadow`; a 2 px floor gives 2.631, matching the
/// gentlest of those while making the far half of the field up to 3x too wide.
/// At 4 px the mid-distance visibly mats into tufts and stops reading as
/// individual blades. The selectivity the trick is sold on does not buy a
/// better trade than simply authoring a slightly wider blade.
///
/// One interaction was tested and ruled out rather than assumed: weighting
/// `widen` by `fade`, so the clamp cannot re-inflate a blade the density
/// falloff is walking to zero, changes the table by at most 0.001. Most of
/// `grass_slope` is at full fade, so there is nothing to fight — the
/// regression is the clamp on its own merits.
///
/// The code stays because it is correct, cheap when off (the branch folds on a
/// literal) and the measurement above is only reproducible if it exists.
static const float GRASS_MIN_PIXELS = 0.0;

[shader("vertex")]
GrassOutput grassVertexMain(uint vertexID: SV_VertexID) {
    uint bladeIndex = vertexID / GRASS_VERTS;
    uint local = vertexID % GRASS_VERTS;
    uint segment = local / 6;
    uint corner = local % 6;

    GrassBlade blade = push.grass[bladeIndex];

    // Which ring and which edge this corner is. Two triangles per segment.
    const uint2 CORNERS[6] = {
        uint2(0, 0), uint2(0, 1), uint2(1, 0),
        uint2(1, 0), uint2(0, 1), uint2(1, 1),
    };
    uint ring = segment + CORNERS[corner].x;
    float edge = CORNERS[corner].y == 0 ? -1.0 : 1.0;
    float t = float(ring) / float(GRASS_SEGMENTS);

    float3 base = blade.position.xyz;
    float height = blade.position.w;
    float2 facing = blade.facing.xy;
    float width = blade.facing.z;
    float tilt = blade.facing.w;
    float bend = blade.shape.x;

    // Rest control points, matching `loom_grass::blade::controls` — the same
    // curve the CPU builds, so the two agree about what a blade looks like.
    float lean = sin(tilt) * height;
    float rise = cos(tilt) * height;
    float3 p1 = float3(facing.x * lean * 0.33 * bend, rise * 0.55, facing.y * lean * 0.33 * bend);
    float3 p3 = float3(facing.x * lean, rise, facing.y * lean);
    float3 p2 = float3((p1.x + p3.x) * 0.5, (p1.y + p3.y) * 0.5 + rise * 0.06, (p1.z + p3.z) * 0.5);

    // **The bend.** Wind is sampled once at the base, not per control point:
    // a blade is a few centimetres tall and sampling along it would cost three
    // field evaluations to produce a difference smaller than the blade. The
    // clump hash phase-shifts it so neighbours do not move in lockstep — the
    // unison artifact, which is the single most recognisable tell in a
    // vegetation shot.
    float seconds = push.environment[0].weather.z;
    float3 air = grassWind(base);
    float phase = frac(blade.shape.z) * 6.2831853;
    float sway = 1.0 + 0.25 * sin(seconds * 1.7 + phase);
    float3 push3 = air * sway;

    // Displacement grows along the blade — the base is rooted, the tip is
    // free. Quadratic, because a linear ramp bends a blade from the root and
    // grass bends from partway up.
    float stiffness = 0.055 / max(height, 0.02);
    float3 p1b = p1 + push3 * (0.33 * 0.33) * stiffness;
    float3 p2b = p2 + push3 * (0.66 * 0.66) * stiffness;
    float3 p3b = p3 + push3 * stiffness;

    // Keep the blade its own length. Without this a strong gust stretches it,
    // which reads as grass growing when the wind picks up.
    float reach = length(p3b);
    if (reach > height) {
        p3b *= height / reach;
    }

    float u = 1.0 - t;
    float3 point = u * u * u * float3(0, 0, 0)
                 + 3.0 * u * u * t * p1b
                 + 3.0 * u * t * t * p2b
                 + t * t * t * p3b;
    float3 tangent = 3.0 * u * u * p1b + 6.0 * u * t * (p2b - p1b) + 3.0 * t * t * (p3b - p2b);
    tangent = length(tangent) > 1e-6 ? normalize(tangent) : float3(0, 1, 0);

    // -----------------------------------------------------------------------
    // **Density falloff with a soft edge**, which is the whole of the
    // non-temporal AA toolkit that survived measurement.
    //
    // Far away most blades are dropped, chosen by a hash of the blade's own
    // position so the survivors never change as the camera moves, and each one
    // leaves by shrinking rather than by vanishing at a threshold.
    //
    // Every number below is `cargo xtask shimmer`'s flicker on `meadow`, at 4x
    // MSAA, each variant differing from its neighbour by one thing:
    //
    //     no cull (every blade)              0.354
    //     hard cull, 12% surviving at range  0.234
    //     + soft fade                        0.214
    //     + fading all the way to none       0.137   <- shipped
    //     soft fade + alpha-to-coverage      0.212
    //     soft fade + minimum-width clamp    0.419
    //
    // **Those absolute values are void and the ranking is unconfirmed.** They
    // were taken before `shimmer` was fixed to use each scene's authored
    // camera, so they photographed a bare slab of soil from 38 m with the
    // falloff having already deleted every blade. `meadow` measures 2.712 on
    // the working instrument, not 0.137. The falloff has not been re-measured
    // against no-falloff since; the table is kept only because it records what
    // was tried and in what order, and re-running it is a cheap loose end.
    //
    // What HAS been re-measured is the last row. See `GRASS_MIN_PIXELS` above:
    // on the working instrument the clamp helps `meadow` monotonically and
    // hurts `grass_slope` at every floor, which is a third negative result
    // rather than the reversal the old numbers made it look like.
    //
    // One thing that did not earn its place and is recorded here so the
    // experiment is not run again:
    //
    //   **Alpha-to-coverage on the fading blade.** 0.002, which is under this
    //   metric's resolution, because the geometric shrink has already taken
    //   the blade's area to zero by the time its opacity would matter. Also
    //   pre-fix, and so worth exactly what the rest of the table is worth.
    // -----------------------------------------------------------------------
    float viewDistance = length(push.environment[0].eye.xyz - base);

    // Blades begin thinning at NEAR and the last of them is gone by FAR. These
    // are metres, not screen space, and that is a simplification with a known
    // ceiling: the right quantity is a blade's *apparent* size, so a narrow FOV
    // or a much taller species will want these retuned rather than inherited.
    const float GRASS_NEAR = 8.0;
    const float GRASS_FAR = 55.0;
    float falloff = saturate((viewDistance - GRASS_NEAR) / (GRASS_FAR - GRASS_NEAR));

    // **Ending at zero rather than at a floor was worth 0.214 -> 0.137.** A
    // floor leaves a fixed fraction of blades scattered across the far field,
    // and a sparse scatter of sub-pixel blades is *noisier* than either a full
    // field or none — every one of them is a lone twinkling pixel with nothing
    // around it to average against. Taking the fraction to zero costs nothing
    // in the authored shot, where the farthest blade is 16m away and barely
    // into the falloff at all, and it is what makes the distant field resolve
    // to flat ground instead of to static.
    //
    // Which only works because the ground under a grass field is authored the
    // colour of grass. That is the other half of density falloff and it is a
    // scene-authoring rule, not an engine one: see the note on `Soil` in
    // `assets/test/meadow.loom`.
    //
    // **The band is in draw space, not metres**, so the same 8% of the
    // population is mid-fade at every range and the transition never becomes a
    // visible ring of short grass. `grassCullDraw` is a stable hash of the
    // blade's own position, so which blades go, and in what order, never
    // changes as the camera moves — only how far each has got through its own
    // exit. Starting the ramp above 1.0 is what keeps near grass untouched: a
    // draw is in [0,1), so a threshold of 1.0 + BAND leaves every near blade
    // above the top of its own band and at full height.
    const float FADE_BAND = 0.08;
    float keep = (1.0 + FADE_BAND) * (1.0 - falloff);
    float fade = saturate((keep - grassCullDraw(blade)) / FADE_BAND);

    // A uniform scale about the base, not a height scale. Scaling height alone
    // would leave a full-width blade lying on the ground at the end of its
    // fade — a little green mat, which is more visible than the pop it
    // replaced. Scaling everything means the blade approaches a point, and a
    // point covers no pixels.
    point *= fade;

    float3 side = float3(-facing.y, 0.0, facing.x);

    // -----------------------------------------------------------------------
    // **Minimum screen-space width**, re-measured on an instrument that can
    // see the grass. See `GRASS_MIN_PIXELS` below for the table and verdict.
    //
    // The measurement is a projection of two world points, never a term of
    // the matrix. An early version took the vertical scale from `mvp[1][1]`,
    // which is the *view*-projection: the blade then carried the camera's
    // rotation and grew into a sheet whenever the camera turned. Projecting
    // `base` and `base + side * width` and measuring the gap between them is
    // exact and does not care how the matrix is laid out.
    //
    // **Measured before the fade, applied as a factor.** A blade mid-fade is
    // sub-pixel by design — the falloff is walking it to zero, which is worth
    // 0.214 -> 0.137 on its own — so clamping its *faded* width would floor
    // exactly the blades the shipped tool is removing. The floor is computed
    // from the blade's intrinsic width and the fade still multiplies to zero.
    // -----------------------------------------------------------------------
    float widen = 1.0;
    if (GRASS_MIN_PIXELS > 0.0) {
        float4x4 viewProj = push.objects[push.objectOffset].mvp;
        float4 clipA = mul(viewProj, float4(base, 1.0));
        float4 clipB = mul(viewProj, float4(base + side * width, 1.0));
        float2 ndcA = clipA.xy / max(clipA.w, 1e-4);
        float2 ndcB = clipB.xy / max(clipB.w, 1e-4);
        float pixelsWide =
            length((ndcB - ndcA) * push.environment[0].viewport.xy * 0.5);
        if (pixelsWide > 1e-4) {
            widen = max(1.0, GRASS_MIN_PIXELS / pixelsWide);
        }
    }

    float halfWidth = width * 0.5 * (1.0 - t * t) * fade * widen;

    // A fully faded blade has zero extent in every direction, so all six of its
    // corners land on `base` and the triangles are degenerate. A vertex shader
    // cannot decline to emit; degenerate triangles are discarded before
    // rasterisation and cost nothing downstream, which is the same trick the
    // hard cull used and the reason no branch is needed here.
    float3 world = base + point + side * halfWidth * edge;

    // **Not the geometric normal**, deliberately — a flat blade lit honestly
    // reads as paper. Tilted outward from the spine and blended toward the
    // ground, more strongly at the base where a blade is most surrounded by
    // its neighbours. Same treatment as the CPU mesh path.
    float3 face = cross(side, tangent);
    float towardGround = (1.0 - t * 0.65) * 0.55;
    float3 normal = normalize(lerp(face, float3(0, 1, 0), towardGround));

    GrassOutput out;
    out.clip = mul(push.objects[push.objectOffset].mvp, float4(world, 1.0));
    out.worldPos = world;
    out.normal = normal;
    out.uv = float2(edge * 0.5 + 0.5, t);
    out.albedo = unpackRGB(blade.shape.w) * blade.shape.y;
    return out;
}

[shader("fragment")]
float4 grassFragmentMain(GrassOutput in) : SV_Target {
    // Both faces are lit as if facing the viewer. Grass has no back, and a
    // blade that turns edge-on and goes black is the harshest artifact in the
    // whole field.
    float3 normal = normalize(in.normal);

    // Wrapped, not clamped: light bleeds around a thin blade rather than
    // stopping dead at the terminator, which is what makes a field read as
    // soft rather than as a million little planes.
    float wrapped = saturate((dot(normal, sunDirection()) + 0.4) / 1.4);

    // **Authored, not hardcoded.** The colour is the `Material` beside the
    // field's `Grass`, shifted per clump toward straw or blue-green and scaled
    // by the blade's own shade — all of it decided on the CPU and delivered in
    // `shape.w`. It was a literal here, and two shipped scenes authored an
    // albedo that nothing read.
    float3 albedo = in.albedo;
    // **Darken toward the base.** The cheapest high-value trick available and
    // the stand-in for the ambient occlusion this renderer does not have: the
    // bottom of a blade is buried in its neighbours and receives almost no
    // sky.
    float occlusion = lerp(0.35, 1.0, in.uv.y * in.uv.y);

    float3 lit = albedo * (ambientStrength() * occlusion + sunStrength() * wrapped * occlusion);
    // A touch of translucency at the tip, where a blade is thin enough to
    // transmit — grass is not opaque and looks wrong when treated as if it is.
    lit += albedo * sunColor() * pow(saturate(dot(-sunDirection(), normalize(in.worldPos - push.environment[0].eye.xyz))), 4.0) * in.uv.y * 0.25;

    // Same haze every other surface gets, so grass sits in the scene rather
    // than floating in front of it.
    float3 fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    return float4(lerp(lit, fogColor(normalize(in.worldPos - push.environment[0].eye.xyz)), fog), 1.0);
}

// ---------------------------------------------------------------------------
// Water
//
// **The surface itself is `loom_sample_water`, generated from the Rust.** This
// file never writes a Gerstner term: `loom_water::slang()` is emitted into
// `generated/water.slang` by `build.rs`, and `loom_water`'s agreement test
// compiles that same string and compares it numerically against the Rust that
// buoyancy will read. Hand-writing the wave sum here is the one mistake that
// puts a boat above the water it is drawn on with no error anywhere.
//
// **The mesh is concentric LOD rings, camera-centred, one draw.** Level 0 is a
// WATER_RES x WATER_RES grid of WATER_CELL cells; each level after it doubles
// the cell size and drops its central quarter, which the level inside covers
// exactly. Rings rather than a real quadtree because a quadtree buys per-tile
// culling that nothing here can spend — there is no indirect draw and no
// visible set — and costs a per-frame traversal on the CPU to decide what a
// vertex shader is already deciding for free.
//
// **A ring mesh IS a screen-space-uniform tessellation, and WATER_RES is the
// only thing that sets how uniform.** A vertex at Chebyshev radius `r` sits in
// a cell of `r / (WATER_RES/2)` metres — proportional to distance, so its
// *angular* size is the constant `2 / WATER_RES` radians no matter how far away
// it is. That is the property a projected grid is usually reached for. See
// ADR 0013: at WATER_RES = 32 the constant was 3.6°, which is 58 pixels of quad
// at 900p, and 58-pixel quads are what "the LOD is way too aggressive" means.
// At 128 it is 0.9°, or about 14 pixels.
//
// **And it ends at the horizon, not at a disc edge.** The outermost level's
// outer vertex ring is pushed to WATER_HORIZON and every clip position is held
// inside the far plane, so there is no radius at which water stops.
//
// **The grid is snapped to the COARSEST cell, so no vertex ever slides.** A
// camera-centred mesh that follows the eye continuously moves every vertex
// every frame, which changes every vertex's displacement every frame, which is
// a shimmer generator. Snapped, the vertices sit on a fixed world lattice and
// the mesh is a window onto it: the eye can move and nothing in the geometry
// moves at all. All levels share one snap because they must share a centre for
// the holes to line up, and the coarsest cell is the only increment every
// level's lattice divides. The cost is that the window jumps 16 m at a time —
// which moves no vertex, only the LOD boundaries, and those are far away by
// construction.
//
// **The quantum has to fit inside level 0, and it did not used to.** The eye
// stands up to half a coarsest-cell from the snapped centre; level 0 reaches
// `WATER_RES/2` cells. At 32 and seven levels that was 16 m of offset against
// 8 m of reach, so the camera spent most of its travel *outside the finest
// ring* and the water under it was drawn by level 1 or 2. ADR 0013 found it in
// `river`, where it read as a mirror. `the_water_draw_matches_the_shader_s_grid`
// now asserts `2^(WATER_LEVELS-1) <= WATER_RES`; at 128 and six it is 32
// against 128.
//
// **No cracks, by construction rather than by skirt.** Each level's outermost
// vertex ring is snapped to the next level's lattice, so the two share exactly
// the same vertices along their shared edge and evaluate the same function at
// them. The fine side's spare vertices collapse onto their neighbours and the
// triangles between them are degenerate, which costs nothing. A skirt would
// hide a gap; this removes it.
// ---------------------------------------------------------------------------

/// Cells per side, per level — **and the screen-space density knob**. A cell at
/// Chebyshev radius `r` is `r / (WATER_RES/2)` metres across, which subtends
/// `2 / WATER_RES` radians at any distance. 128 is 0.9°, about 14 pixels of
/// quad at 900p through a 55° lens. It was 32, which is 58 pixels.
///
/// The cost is quadratic and the benefit is linear, because a ring covers a
/// whole disc while a camera sees a wedge of it: six levels of 128² quads is
/// 589,824 vertices against 43,008. ADR 0013 measures what that costs and the
/// answer is that it is still nothing.
static const uint WATER_RES = 128;
/// How many rings of real geometry. Level L has cells of `WATER_CELL * 2^L`, so
/// the outermost reaches `WATER_RES/2 * WATER_CELL * 2^(L-1)` metres — 1024 m
/// here, past the 1000 m far plane, and past the ~830 m where the Nyquist fade
/// below has retired the last wave `ocean` authors. Rings beyond the point
/// where the sea is drawn flat buy nothing the skirt does not.
static const uint WATER_LEVELS = 6;
/// The finest cell, in metres, right under the camera.
static const float WATER_CELL = 0.5;
/// Where the outermost ring's outer edge goes, in metres.
///
/// **This is the horizon, and it is why the water has no edge.** The mesh used
/// to be a 512 m disc, so there was a radius where the sea simply stopped: a
/// hard horizontal line, measured at 8/255 per channel against the sky in
/// `ocean` at its authored camera even with fog at 85%. Any finite radius has
/// that line somewhere; what makes it invisible is putting it far enough out
/// that it falls under the horizon by less than a pixel *for any camera height
/// a scene might use*. 50 km subtends 0.11° from 100 m up — a fifth of a pixel
/// — and the atmosphere has long since saturated by then.
///
/// It is not infinity because infinity does not survive the projection: a point
/// at infinite distance lands at `z/w = far/(far-near)`, just outside the far
/// plane, and is clipped. A finite skirt plus the clamp in `waterVertexMain` is
/// the same picture without an infinite-projection matrix the rest of the
/// renderer would have to agree to.
static const float WATER_HORIZON = 50000.0;

/// **The Nyquist fade, and the one place the drawn surface leaves the sampled
/// one.** See ADR 0012.
///
/// A wave shorter than about twice the cell sampling it cannot be represented
/// by that cell — the vertices land wherever the lattice happens to cut it, and
/// what rasterises is a flat quad or a stepped shelf whose height is an
/// artifact of the sampling phase rather than of the sea. W4 measured this at
/// roughly 43% of `ocean`'s flicker. So each wave's amplitude is taken to zero
/// as the samples per wavelength fall toward the limit.
///
/// **This lives here and not in `loom_water`**, because it is a property of
/// THIS MESH's sampling rate and of nothing else. `sample_water` — the function
/// buoyancy reads, the function `loom water --at` prints, the function the
/// agreement test pins — is untouched and still answers with the whole sea. A
/// boat floats on the surface the CPU knows about; past the fade's onset the
/// far field is drawn smoother than that surface is. The alternative is not a
/// truer picture: at three samples per wavelength the drawn surface already
/// differs from `sample_water` by most of the wave's amplitude, and differs
/// *incoherently*, which is the flicker.
///
/// The two constants are estimated samples per wavelength (`λ / cell`):
/// - `WHOLE` 4 — a sine reconstructed piecewise-linearly at four samples per
///   period misses its crest by 29% of amplitude. That is a visible
///   approximation and it is the right trade, because the alternative below is
///   worse than an approximation.
/// - `GONE` 2 — Nyquist exactly. Below it the sampled height depends entirely
///   on where the lattice falls relative to the crest, which is not a wave any
///   more, it is noise keyed to the grid.
///
/// **These were 8 and 3, and that was wrong in the near field.** The finest
/// cell is `WATER_CELL` = 0.5 m, so `WHOLE = 8` meant *no wave shorter than
/// 4 m was ever drawn whole, at any distance, including directly in front of
/// the camera*. `river.loom` authors 3.4 m and 1.9 m ripples; the 1.9 m wave
/// came out at **6.9% of its amplitude at point-blank range** and the river
/// rendered as a mirror. The human saw it and asked.
///
/// The error was treating "eight samples per period looks smooth" as a floor
/// rather than as the point above which no fade is needed. Deleting a wave the
/// mesh can represent imperfectly is worse than drawing it imperfectly: a 1.9 m
/// ripple at 3.8 samples per period is visibly a ripple, and absent it is
/// visibly a mirror. The fade exists to retire waves the mesh genuinely cannot
/// sample, and Nyquist is where that line actually is.
///
/// **Both constants survived ADR 0013 unchanged, and that is the test of them.**
/// They are stated in samples per wavelength, which is a fact about
/// reconstruction and not about this mesh; the mesh enters only through
/// `waterSamplingCell`, which is written against `WATER_RES`. Quadrupling the
/// density therefore moved the whole curve outward without touching a number
/// here: on `ocean` the fade first touches a wave at 37 m instead of 9.4 m, and
/// the sea is drawn flat past ~830 m instead of past ~140 m.
static const float WATER_FADE_WHOLE = 4.0;
static const float WATER_FADE_GONE = 2.0;

/// The size of the cell sampling this point, as a function of **position only**.
///
/// It must be position-only for two reasons. The seam: a vertex on a ring
/// boundary is emitted by both levels, at the same `xz` by construction, and if
/// the two disagreed about the fade the watertight edge W4 built would open
/// again. And continuity: a weight that stepped per level would trade facets
/// for a visible ring where the amplitude jumps.
///
/// Level `L` covers Chebyshev radius `(WATER_RES/4)·cell_L` to
/// `(WATER_RES/2)·cell_L` from the centre, so `r / (WATER_RES/2)` is the exact
/// cell size at each ring's outer edge and under-reads by at most 2× inside it.
/// Under- rather than over-reading is the deliberate half: clamped at
/// `WATER_CELL` it is **exact everywhere level 0 draws** (`r ≤ 32 m`), so the
/// near field is bit-identical to the unfaded surface for any wave longer than
/// `WATER_FADE_WHOLE · WATER_CELL` = 4 m.
///
/// **Written against `WATER_RES` rather than against 16, and that is load
/// bearing.** ADR 0013 changed the density by changing `WATER_RES` alone; a
/// hand-inlined 16 here would have left the fade describing a mesh that no
/// longer exists, which is the failure this expression is shaped to make
/// impossible.
///
/// Measured from the SNAPPED centre, not the eye. The eye moves continuously
/// and every vertex's amplitude would move with it — the same shimmer generator
/// the 32 m snap exists to avoid. Against the centre a vertex's weight is
/// constant until the window jumps, which is when its LOD level changes anyway.
float waterSamplingCell(float2 xz, float2 centre) {
    float2 offset = abs(xz - centre);
    return max(WATER_CELL, max(offset.x, offset.y) / (float(WATER_RES) * 0.5));
}

/// **The Nyquist fade for a cascade, which is a band rather than a wave** — one
/// amplitude weight per cascade, in the lanes `loom_ocean_at` multiplies by.
///
/// The sixteen-wave path fades each wave on its own wavelength, because it has
/// one. A cascade does not: it carries every wavenumber in `[k_lo, k_hi)` at
/// once, so a per-wave weight is not expressible and one number per cascade is
/// the only shape available. This keys on **the longest wavelength the cascade
/// carries** — `2π/k_lo`, uploaded as `oceanLongest` — through the same two
/// constants and the same `waterSamplingCell` the wave path uses.
///
/// **The cost of that choice, stated because it is a choice.** Keying on the
/// long end means a cascade is drawn where the local cell cannot resolve its
/// *short* end, so the short end of every band aliases; keying on the short end
/// instead is worse in a way that is not a trade at all, because the chop
/// cascade's Nyquist wavelength is 0.5 m against this mesh's finest 0.5 m cell,
/// so it would be weighted to zero **everywhere, including at the camera** — a
/// third of the sea deleted rather than drawn imperfectly. That is the same
/// judgement `WATER_FADE_WHOLE`'s own comment records after the river came out
/// as a mirror: deleting detail the mesh can represent imperfectly is worse than
/// drawing it imperfectly.
///
/// For `shipping_stack` at n = 128 the weights come out: swell (longest 2048 m)
/// whole everywhere and only fading past the last real ring; sea (32 m) whole to
/// about 512 m of Chebyshev radius; chop (4 m) whole to about 64 m and gone by
/// 128 m. The 50 km skirt is far enough out that all three are near zero, which
/// is what keeps the wrapped tile from drawing noise onto the horizon.
///
/// `ponytail:` a cascade is gated on one end of its band. The honest version
/// weights it by the *fraction of its band* the local cell can resolve —
/// `(k_resolvable − k_lo)/(k_hi − k_lo)` against the spectrum's own energy
/// distribution — which fades the short end out of a cascade instead of taking
/// the whole cascade with it. Build that if the aliasing this leaves in
/// measurably matters; `cargo xtask shimmer` and `loom salt` are what say so.
float4 waterCascadeFade(LoomOcean sea, float cell) {
    float4 weight = float4(0.0, 0.0, 0.0, 0.0);
    for (int c = 0; c < LOOM_OCEAN_MAX_CASCADES; ++c) {
        if (c >= sea.count) { break; }
        weight[c] = smoothstep(
            WATER_FADE_GONE, WATER_FADE_WHOLE, push.environment[0].oceanLongest[c] / cell);
    }
    return weight;
}

struct WaterOutput {
    float4 clip : SV_Position;
    float3 worldPos : TEXCOORD0;
    float3 normal : TEXCOORD1;
    /// Still-water depth under this point, in metres. Negative is dry land.
    ///
    /// **Interpolated, and that is the shoreline.** It used to be
    /// `nointerpolation` because it was a constant; now it varies across a
    /// quad, and the fragment shader discarding where it goes negative is what
    /// cuts the water off at the water's edge rather than at the nearest
    /// vertex. The cut then has cell-size-independent resolution, which
    /// matters because the cell out at the far LOD ring is 32 m across.
    float depth : TEXCOORD2;
    /// How near this point is to breaking: `Σ Q·k·A·sin φ` from the **faded**
    /// wave set, so it describes the surface actually rasterised rather than
    /// the one the scene authored. Whitecaps read it.
    ///
    /// **Interpolated across cells up to 16 m wide out in the far rings.** In
    /// the near field the cell is 0.5 m and the per-pixel noise erosion hides
    /// the piecewise-linear underlay entirely; far out it is a smear, and the
    /// honest answer is that foam is not the right tool at that range. Stated
    /// here rather than discovered later.
    float fold : TEXCOORD3;
    /// How much foam this point is still carrying from crests that have
    /// already passed: the trail. See `FOAM_TRAIL_STEP`.
    float foamHist : TEXCOORD4;
    /// The **undisplaced** xz this vertex was evaluated at.
    ///
    /// Carried so the fragment shader can sum the wavelet events itself. Not
    /// `worldPos.xz`: the Gerstner pinch moves the vertex sideways after the
    /// sum, and asking where the vertex ended up is the circular definition the
    /// vertex shader already refuses. The base point is linear across a
    /// triangle, so interpolating it is exact.
    float2 baseXZ : TEXCOORD5;
    /// The wavelet slope this vertex used, `(∂h/∂x, ∂h/∂z)`.
    ///
    /// **Carried so the per-pixel slope can be added as a difference rather
    /// than as a second copy.** `normal` already has this term baked into it
    /// by `loom_sample_water`; subtracting what the vertex knew and adding what
    /// the pixel knows recovers the resolution without counting the wake twice.
    float2 waveletSlope : TEXCOORD6;
    /// **The breaking criterion** — `LoomWaterSample::mu_max`, the largest
    /// eigenvalue of the horizontal compression rather than its trace.
    ///
    /// Every foam threshold reads this and not `fold`. They are the same number
    /// on a sea running one way and they are not on a crossing one, where the
    /// trace calls a twelfth of the surface broken that has folded on neither
    /// axis — `loom_water::WaterSample::mu_max` carries the measurement.
    ///
    /// `fold` beside it is kept for `crest`, which asks about the surface's
    /// *shape* (a trough sees less sky than a crest) and is signed. Breaking
    /// and shape are two questions and the eigenvalue only answers one.
    float breakMu : TEXCOORD7;
    /// The direction the surface is folding along, world xz, unnormalised
    /// across the triangle. Foam streaks lie along it — ADR 0053 §7 deleted the
    /// global-wind frame this replaces.
    float2 breakDir : TEXCOORD8;
    /// **The falling sheet, and zero on every analytic surface vertex** — ADR
    /// 0054.
    ///
    /// `x` is the flag the fragment shader branches on — **`> 0.5` a falling
    /// sheet, `< -0.5` the cinematic tier's marched free surface, `0.0` the
    /// analytic sea** (see `WATER_CINEMATIC_REFLECT_ROUGH`); `y` the aeration
    /// at this depth; `z` how far below the lip it is, in metres; `w` the
    /// **departure time** of the water here — the clock reading when this
    /// parcel left the brink.
    ///
    /// That last one is the whole streak model. Texturing against depth would
    /// make a pattern nailed to the air; texturing against departure time makes
    /// one nailed to the *water*, and it stretches as it falls for free,
    /// because a fixed interval of departure time covers more metres the faster
    /// the sheet is going. No scroll speed is authored anywhere.
    float4 nappe : TEXCOORD9;
};

/// Quads across the lip, and down the fall. 32 x 28.
///
/// Chosen by look rather than by cost: the strip is 5,376 vertices against the
/// surface's 589,824, so it is inside the noise of the draw it rides in. Across
/// the lip it only has to be smooth; down the fall it has to resolve the
/// throw's parabola near the lip, which the bias below is what actually buys.
static const uint NAPPE_COLS = 32;
static const uint NAPPE_ROWS = 28;

/// How the rows are packed down the fall: `Z = drop · (v)^NAPPE_BIAS`.
///
/// **Above 1, so rows crowd toward the lip.** Everything that changes fastest —
/// the throw's curvature, the thickness, the aeration going from nothing to
/// most of the sheet — happens in the first metre, and a uniform `v` spends
/// most of its rows in the straight part where nothing does. 1.3 is gentle
/// enough that the far rows are still under a metre apart on a ten-metre fall.
static const float NAPPE_BIAS = 1.3;

/// Vertices in the surface's rings, after which the nappe's strip begins.
///
/// **One draw, two meshes, and this is the seam.** `WATER_VERTS` in
/// `renderer.rs` is this plus the strip; a mismatch draws the top of the
/// waterfall as a stray water quad, which is why a test pins both.
static const uint WATER_SURFACE_VERTS = WATER_RES * WATER_RES * WATER_LEVELS * 6;

/// The falling sheet's vertex, from its index within the strip.
///
/// Everything about the shape comes out of `loom_nappe_at`, generated from
/// `loom_water::nappe` — never hand-written here, because the nappe is
/// force-capable and a shader-only copy of the hydraulics is exactly the
/// divergence that puts a character in the wrong part of a waterfall.
WaterOutput nappeVertex(uint nappeID) {
    uint quad = nappeID / 6;
    uint corner = nappeID % 6;
    const uint2 CORNERS[6] = {
        uint2(0, 0), uint2(0, 1), uint2(1, 0),
        uint2(1, 0), uint2(0, 1), uint2(1, 1),
    };
    uint cu = quad % NAPPE_COLS;
    uint cv = quad / NAPPE_COLS;
    float u = float(cu + CORNERS[corner].x) / float(NAPPE_COLS);
    float v = float(cv + CORNERS[corner].y) / float(NAPPE_ROWS);

    float3 lipA = push.environment[0].cascadeA.xyz;
    float3 lipB = push.environment[0].cascadeB.xyz;
    float discharge = push.environment[0].cascadeA.w;
    float2 fall = push.environment[0].cascadeC.xy;
    float drop = push.environment[0].cascadeC.w;

    WaterOutput out;
    // **Collapsed to a point when there is no cascade**, the same trick the
    // covered centre of a water ring uses: a vertex shader cannot decline to
    // emit, and a degenerate triangle is discarded before rasterisation for
    // free. Every scene in the repository takes this branch.
    if (discharge <= 0.0 || drop <= 0.0) {
        out.clip = mul(push.objects[push.objectOffset].mvp, float4(lipA, 1.0));
        out.worldPos = lipA;
        out.normal = float3(0.0, 1.0, 0.0);
        out.depth = 1.0;
        out.fold = 0.0;
        out.foamHist = 0.0;
        out.baseXZ = lipA.xz;
        out.waveletSlope = float2(0.0, 0.0);
        out.breakMu = 0.0;
        out.breakDir = float2(0.0, 0.0);
        out.nappe = float4(0.0, 0.0, 0.0, 0.0);
        return out;
    }

    LoomNappe nappe = loom_nappe_brink(discharge, push.environment[0].cascadeB.w,
                                       push.environment[0].cascadeC.z);
    // **`drop` is the lip's height above the still-water line, so the last row
    // lands exactly ON the pool.** Carrying the strip further, so its free edge
    // would be occluded by the surface in front of it, was built and measured
    // and is a no-op: quarter of a metre of overshoot moves 162 pixels of
    // `spout` and 0 of `cascade`, and two metres moves the same 105. The sheet
    // is a *curve*, so extending `v`'s range only reparametrises where the rows
    // sit on it. The straight line across the foot is the sheet meeting a
    // glassy pool, not a free edge — see `NAPPE_TOE`.
    float z = drop * pow(v, NAPPE_BIAS);
    LoomNappeAt at = loom_nappe_at(nappe, z);

    float3 base = lerp(lipA, lipB, u);
    float3 world = base
        + float3(fall.x, 0.0, fall.y) * at.throwX
        + float3(0.0, -z, 0.0);

    out.clip = mul(push.objects[push.objectOffset].mvp, float4(world, 1.0));
    out.worldPos = world;
    // **The sheet's face, tilted back along the trajectory.** Purely horizontal
    // reads as a pane of glass hung in the air; leaning it into the fall by the
    // ratio of the two velocities is the surface's real slope and costs a
    // normalize. `V_i/V_j` is 1 at the lip and goes to zero as the sheet turns
    // vertical, which is exactly how the face turns to meet the eye.
    float lean = nappe.exit_speed / at.speed;
    out.normal = normalize(float3(fall.x, lean, fall.y));
    // Positive so the shoreline discard in `waterFragmentMain` is not what
    // decides whether a waterfall is drawn; the nappe branch runs before it.
    out.depth = 1.0;
    out.fold = 0.0;
    out.foamHist = 0.0;
    // The lip parameter, so the fragment can put strings across the sheet. It
    // rides in `baseXZ` because a nappe fragment never sums the events.
    out.baseXZ = float2(u, 0.0);
    out.waveletSlope = float2(0.0, 0.0);
    out.breakMu = 0.0;
    out.breakDir = float2(0.0, 0.0);
    // **Departure time, from the jet velocity rather than from a clock.** Free
    // fall gives `t_fall = (V_j − V_i)/g` exactly, so this parcel left the
    // brink that long ago. See `WaterOutput::nappe`.
    float departure = push.environment[0].weather.z - (at.speed - nappe.exit_speed) / 9.81;
    out.nappe = float4(1.0, at.aeration, z, departure);
    return out;
}

/// The cinematic tier's free surface — ADR 0057 addendum.
///
/// **A vertex fetch and nothing else.** Every other water vertex in this file
/// is *generated* — the rings from `SV_VertexID`, the nappe from the
/// hydraulics — because their shape is a closed form. This one's shape is what
/// a GPU fluid solver actually did, marched into triangles by
/// `loom_render::fluid_surface` on the CPU, so the only honest thing here is to
/// read it.
///
/// **It shares `waterFragmentMain`, which is the entire point of the slice.**
/// Cinematic water gets the same two-leg Snell refraction, the same
/// per-channel Beer-Lambert column, the same roughness-aware Fresnel, the same
/// traced reflection ray with its sky fallback and the same 4x MSAA as the
/// analytic sea. There is no second water look in this engine and there is not
/// going to be one.
///
/// The varyings the analytic surface derives from its wave sum are set to the
/// values that mean "no wave here": `fold` and `breakMu` zero, so no whitecap
/// threshold fires and `crest` is zero; `waveletSlope` zero, so the per-pixel
/// wake difference is nothing; `nappe.x` zero, so the falling-sheet branch is
/// not taken. `depth` is the water's own height above the domain floor and is
/// positive everywhere, which is what keeps the shoreline discard - a query
/// against a terrain height field this water does not stand on - from cutting
/// holes in a tank.
[shader("vertex")]
WaterOutput fluidVertexMain(uint vertexID: SV_VertexID) {
    LoomFluidVertex v = push.environment[0].fluidVertices[vertexID];
    WaterOutput out;
    out.clip = mul(push.objects[push.objectOffset].mvp, float4(v.position.xyz, 1.0));
    out.worldPos = v.position.xyz;
    out.normal = v.normal.xyz;
    out.depth = v.position.w;
    out.fold = 0.0;
    // The whitewater the CPU deposited at this vertex - the readback's
    // hull-waterline and submersion deposits (ADR 0057 addendum). It arrives as
    // a coverage in the same units `instant` and the foam field use, so the
    // fragment shader's `max` of the three needs no new branch.
    out.foamHist = v.normal.w;
    out.baseXZ = v.position.xz;
    out.waveletSlope = float2(0.0, 0.0);
    out.breakMu = 0.0;
    out.breakDir = float2(1.0, 0.0);
    // **`x` is negative here, which says "cinematic free surface"** — see
    // `WATER_CINEMATIC_REFLECT_ROUGH`. It stays in the lane that already
    // answers "what kind of water is this": `> 0.5` a falling sheet, `< -0.5`
    // a marched volume, `0.0` the analytic sea. Putting the flag in `y` would
    // have been a lane documented as *aeration* carrying a boolean, which is
    // the kind of reuse that reads correctly and breaks later.
    out.nappe = float4(-1.0, 0.0, 0.0, 0.0);
    return out;
}

[shader("vertex")]
WaterOutput waterVertexMain(uint vertexID: SV_VertexID) {
    // The falling sheet rides in the same draw, past the surface's rings —
    // ADR 0054. It shares this entry point because it wants the same pipeline,
    // the same multisampled attachments and the same depth test against the
    // pool it pours into.
    if (vertexID >= WATER_SURFACE_VERTS) {
        return nappeVertex(vertexID - WATER_SURFACE_VERTS);
    }
    uint quad = vertexID / 6;
    uint corner = vertexID % 6;
    uint level = quad / (WATER_RES * WATER_RES);
    uint cell = quad % (WATER_RES * WATER_RES);
    uint cx = cell % WATER_RES;
    uint cz = cell / WATER_RES;

    const uint2 CORNERS[6] = {
        uint2(0, 0), uint2(0, 1), uint2(1, 0),
        uint2(1, 0), uint2(0, 1), uint2(1, 1),
    };
    int ix = int(cx + CORNERS[corner].x);
    int iz = int(cz + CORNERS[corner].y);

    // The stitch. On this level's outer boundary, round the coordinate that
    // runs ALONG the boundary down to an even index — which is a vertex of the
    // next level out, whose cells are twice as wide. WATER_RES/2 is even, so
    // index parity and world-lattice parity agree.
    if (ix == 0 || ix == int(WATER_RES)) { iz -= iz & 1; }
    if (iz == 0 || iz == int(WATER_RES)) { ix -= ix & 1; }

    // The central quarter of every level but the first is covered by the level
    // inside it. Collapsed to a point rather than skipped: a vertex shader
    // cannot decline to emit, and a degenerate triangle is discarded before
    // rasterisation for free — the same trick the grass fade uses.
    bool covered = level > 0
        && cx >= WATER_RES / 4 && cx < 3 * WATER_RES / 4
        && cz >= WATER_RES / 4 && cz < 3 * WATER_RES / 4;

    float cellSize = WATER_CELL * exp2(float(level));
    float coarsest = WATER_CELL * exp2(float(WATER_LEVELS - 1));
    float2 centre = round(push.environment[0].eye.xz / coarsest) * coarsest;
    float2 offset = (float2(float(ix), float(iz)) - float(WATER_RES) * 0.5) * cellSize;

    // **The horizon skirt: the outermost vertex ring, moved out to
    // WATER_HORIZON.** Every vertex on that ring has Chebyshev radius exactly
    // `WATER_RES/2 * cellSize`, so one scale factor lands the whole ring on the
    // horizon and keeps it a square — no per-vertex direction maths, and the
    // two triangles of a boundary quad still share both of their outer
    // vertices, which is what keeps it watertight.
    //
    // The quads it stretches are drawn dead flat and that is not luck: at this
    // radius `waterSamplingCell` reads hundreds of metres, so every weight
    // below is *exactly* zero and `loom_sample_water` returns the plane. The
    // ring inside it is at 1024 m, where the same is already true — so there is
    // no step in height across the join, only a step in triangle size, out
    // past the far plane where nothing is drawn anyway.
    //
    // Computed after the stitch, because the stitch can move an index onto the
    // boundary (`iz == 1` next to `ix == 0` rounds down to 0) and a vertex the
    // stitch put on the edge is on the edge.
    if (level == WATER_LEVELS - 1
        && (ix == 0 || ix == int(WATER_RES) || iz == 0 || iz == int(WATER_RES))) {
        offset *= WATER_HORIZON / (float(WATER_RES) * 0.5 * cellSize);
    }

    float2 xz = centre + offset;

    float surface = push.environment[0].water.x;
    // **The depth query, per vertex, out of the same grid the CPU reads.**
    // `loom_ground_height` is generated from `loom_voxel::heightfield`, whose
    // Rust half answers the buoyancy solver — one bake, two lookups, and the
    // agreement test compares them numerically rather than trusting this.
    float ground = loom_ground_height(terrainField(), xz);

    // **The Nyquist fade, applied as an amplitude and nothing else.** Every
    // term in `loom_sample_water` reads `amplitude` — the vertical offset, the
    // horizontal Gerstner pinch, `ka` in the normal, the orbital velocity — so
    // scaling it here means the normal describes the surface that is actually
    // drawn rather than the one that would have been. Shading that disagreed
    // with the geometry is the way a fade like this usually goes wrong.
    //
    // It also cannot break the fold limit the validator proves: `Σ Q·k·A ≤ 1`
    // is linear in `A` and every weight is in `[0, 1]`, the same argument
    // `shoal` already rests on.
    LoomWaveSet faded = push.environment[0].waves;
    float sampleCell = waterSamplingCell(xz, centre);
    for (int w = 0; w < LOOM_MAX_WAVES; ++w) {
        if (w >= faded.count) { break; }
        faded.waves[w].amplitude *= smoothstep(
            WATER_FADE_GONE, WATER_FADE_WHOLE, faded.waves[w].wavelength / sampleCell);
    }

    // **The events, summed at the base point rather than at the displaced
    // one.** The Gerstner pinch moves this vertex sideways *after* the sum, so
    // asking where the vertex ends up would be a circular definition — and it
    // would disagree with the CPU, which asks at the pontoon's own xz. One base
    // point, one sum, both sides.
    LoomWavelet waveletAt = loom_wavelets_at(
        waveletPool(), xz, push.environment[0].weather.z);
    float3 waveletSurface = float3(waveletAt.height, waveletAt.slope.x, waveletAt.slope.y);

    // **The wave model, and the only branch in this shader that picks one** —
    // ADR 0076. A body that asked for the spectrum is displaced by the CPU's own
    // cascade tiles; every other body sums the sixteen waves, character for
    // character as it did before this branch existed.
    //
    // The two are not alternatives at a point: `wave_model = "spectrum"` is
    // refused at load beside an authored wave list, so a spectrum body's
    // `faded.count` is zero and the sum below would draw a flat sea. That is why
    // this is a branch rather than an addition.
    LoomOcean sea = oceanCascade();
    // **The ablation switch for the whole FFT sea** — `crates/loom_render/src/ablate.rs`.
    // Zeroing the count is the one edit that removes the cascade *as a rendering path*
    // rather than dimming it: the branch below falls to the Gerstner sum, which a spectrum
    // body has no waves for, so the surface goes flat. Zero on every ordinary run.
    if (ablated(LOOM_ABLATE_OCEAN_SPECTRUM)) { sea.count = 0; }
    LoomWaterSample surfaceAt;
    if (sea.count > 0) {
        // **The last tick's tile, not a re-evaluation at frame time.** The
        // cascade is stateless — ADR 0076's whole argument — so evaluating it at
        // the frame's own clock would be *legal*, and it would cost a full
        // `evolve` at 3.710 ms of a 16.67 ms tick, on the CPU, per frame. The
        // tile the simulation already built is free. What that costs is written
        // up in the task-3 report with the judder measurement beside it.
        surfaceAt = loom_sample_cascade(
            loom_ocean_at(sea, xz, waterCascadeFade(sea, sampleCell)),
            // No orbital velocity: `render_tiles` does not upload the three
            // velocity tiles, because nothing on this side reads a water
            // velocity — the same reason the flow grid below is zero.
            float3(0.0, 0.0, 0.0),
            surface,
            ground,
            float3(0.0, 0.0, 0.0),
            waveletSurface);
    } else {
    surfaceAt = loom_sample_water(
        faded,
        surface,
        ground,
        xz,
        push.environment[0].weather.z,
        // **No current on the GPU, and it costs nothing here.** The flow field
        // only ever reaches `velocity`, which this shader does not read: the
        // surface a river is drawn at is the surface a still lake is drawn at,
        // and what the current does is move what floats on it. Uploading the
        // flow grid would be a second buffer feeding a value nothing samples.
        // A flow-map material — a river whose *texture* scrolls downstream —
        // is what would need it, and that is river mesh shaping, not W8.
        float3(0.0, 0.0, 0.0),
        waveletSurface);
    }

    // The horizontal displacement is half of what makes a Gerstner wave read
    // as water rather than as a sine: crests pinch toward each other. It is
    // also why the seam stays watertight — both sides of a shared edge
    // evaluate the same function at the same xz and land on the same point.
    float3 world = float3(
        xz.x + surfaceAt.displacement.x,
        surfaceAt.height,
        xz.y + surfaceAt.displacement.z);
    if (covered) {
        world = float3(centre.x, surface, centre.y);
    }

    WaterOutput out;
    out.clip = mul(push.objects[push.objectOffset].mvp, float4(world, 1.0));
    // **Held inside the far plane, which is what lets the skirt reach 50 km
    // through a projection that ends at 1000 m.** Without this the clipper cuts
    // the water at the far plane and the disc edge comes back, one kilometre
    // further out; with it the horizon ring rasterises at the far depth, behind
    // every other surface in the scene, which is where the horizon belongs.
    //
    // Guarded on `w > 0` and only ever clamping *down*. A vertex behind the eye
    // has `w < 0`, and there `w * 0.999999` is the larger number — an
    // unconditional `min` would drag such a vertex's `z` toward it and corrupt
    // the near-plane clip, which is the one this must not touch. Strictly
    // inside rather than equal to `w` because the depth test is LESS against a
    // buffer cleared to 1.0, and a horizon at exactly 1.0 draws nothing.
    if (out.clip.w > 0.0) {
        out.clip.z = min(out.clip.z, out.clip.w * 0.999999);
    }
    out.worldPos = world;
    out.normal = surfaceAt.normal;
    // **Clamped, and not for tidiness.** Off the height grid the depth is the
    // sentinel's — a billion metres — and this attribute is interpolated. A
    // triangle with one vertex inside the grid and one outside then perspective-
    // divides a 6 and a 1e9 together, which loses enough of the small end to
    // come out *negative* on scattered pixels; the shoreline test below
    // discarded them, and the grid's boundary drew itself in the sea as a
    // dotted line. Everything downstream saturates by 5 m, so a hundred is far
    // past any use of the value and interpolates exactly.
    out.depth = min(surfaceAt.depth, WATER_DEPTH_SENTINEL);
    out.fold = surfaceAt.fold;
    out.breakMu = surfaceAt.mu_max;
    out.breakDir = surfaceAt.break_dir;

    // **Foam outlives the crest that made it, and that is what makes it foam.**
    //
    // `in.fold` says how near this point is to breaking *now*, so a whitecap
    // painted from it alone is a highlight welded to the wave: it appears when
    // the crest steepens, vanishes the instant the crest passes, and never
    // sits on the water. Real whitecaps are entrained air. The crest makes
    // them in a second and they take ten to go, drifting downwind while they
    // do — a foam field is a *record* of where crests have been.
    //
    // **Closed form, not an accumulation buffer.** The obvious implementation
    // is a texture that decays each frame, which is GPU state and takes ADR
    // 0045 clause 3's whole burden: seeding, one-dispatch catch-up to
    // `--sim N`, the `repeat` gate, a warm-up. None of that is needed, because
    // `mu_max` is a pure function of `(x, t)`: the recurrence
    // `F(x,t) = max(f(x,t), d·F(x - v·dt, t - dt))` unrolls into a maximum
    // over a few past samples of a function we can simply evaluate. Ten taps
    // buy the same 3.3 s of trail, sampled fine enough to be one trail rather
    // than three whitecaps. See ADR 0049 and its amendment.
    //
    // Sampled at `xz - drift·tau` because a patch made `tau` ago was made
    // upwind of where it has drifted to. The ground height is this column's:
    // the taps are under a metre away and re-marching the height grid three
    // times to move the shoaling by nothing is the wrong trade.
    float2 trailWind = normalize(push.environment[0].wind.xy + float2(1e-4, 0.0))
                     * push.environment[0].wind.z * FOAM_TRAIL_DRIFT;
    float decay = 1.0;
    float trail = 0.0;
    // **A spectrum sea has no closed-form trail, and this is not a shortcut** —
    // ADR 0076. Every tap below asks the surface for `mu_max` at `t - tau`, and
    // the argument that makes that legal is that `mu_max` is a pure function of
    // `(x, t)` the shader can simply evaluate. The cascade is not: the uploaded
    // tiles are ONE instant, so there is no past in them to sample, and the ten
    // taps would silently read the present ten times and call it a trail.
    //
    // What carries foam history for a spectrum body instead is the advected CPU
    // field (ADR 0055), which is stepped inside the fixed step and read through
    // `foamField()` in the fragment shader. `ocean_fft` reads 0.886 coverage at a
    // crest through it. The instantaneous `breakMu` above is the other half.
    if (sea.count == 0) {
    [unroll]
    for (int tap = 1; tap <= FOAM_TRAIL_TAPS; ++tap) {
        float tau = float(tap) * FOAM_TRAIL_STEP;
        decay *= FOAM_TRAIL_DECAY;
        // **No wavelets on the trail taps, and this is now a cost decision
        // rather than an impossibility.** The grid this replaced was a snapshot
        // of one tick and had no past to ask about; an event pool does — every
        // packet is a closed form in `(r, tau)` and `t - tau` is exactly as
        // cheap as `t`. It is skipped because it is FOAM_TRAIL_TAPS more pool
        // sums per vertex for foam the hull and impact deposits already lay.
        LoomWaterSample past = loom_sample_water(
            faded, surface, ground, xz - trailWind * tau,
            push.environment[0].weather.z - tau,
            float3(0.0, 0.0, 0.0),
            float3(0.0, 0.0, 0.0));
        // **The same wander the fragment shader applies — SEA-REBUILD §4.3.**
        // The trail is the DOMINANT coverage term on a Gerstner sea: rendering
        // `float4(instant, field, in.foamHist, 1)` on `whitecaps --sim 400`
        // comes back almost entirely blue. Leaving it on the flat threshold
        // would have fixed the silhouette on the two spectrum scenes and left
        // the reference whitecap field exactly as polygonal as it was — and a
        // constant this term pays is a constant the whole scene pays.
        //
        // At the tap's own place and time, because that is where and when the
        // crest that left this foam broke.
        //
        // **The patch octave only, and the cell octave is not here at all.**
        // That octave exists to bend a boundary *inside* one mesh cell, which
        // is a question only a fragment can ask: it is a 0.71 m lattice and
        // this is evaluated at a 0.5 m vertex spacing, so at vertex rate it is
        // under Nyquist and would be per-vertex noise rather than a field.
        // Taking it at its own mean instead was built first and is worse than
        // useless — a flat +0.0175 on the threshold, wandering nothing and
        // costing coverage everywhere. `cargo xtask ablate`'s
        // `whitecaps/water_foam` row is what measured it: **14.889% with that
        // constant against a 15.0% floor, which is the gate it failed.**
        //
        // Gated on the base threshold for the fragment shader's reason: the
        // wander only ever raises, so a tap under it contributes nothing under
        // any wander, and the tap is then ten `loom_value_noise` calls a vertex
        // that never had to happen.
        //
        // **`WATER_FOAM_BREAK`/`_BROKEN`, and NOT `WATER_FOAM_WET`/`_BREAK` —
        // remembering wetness is what turned the sea white.**
        //
        // `FoamField`'s own `FOAM_CREST_BREAK` says the sentence this used to
        // get wrong: *"`WATER_FOAM_BREAK` = 0.33 is where a crest is drawn
        // white now; where it leaves a raft still there eight seconds later is
        // a different question, and using one number for both is what turns a
        // sea white."* This asked the memory question with the number one rung
        // BELOW the one it warns about — `WATER_FOAM_WET` = 0.22, which is
        // *wet*, not broken — so a crest that merely steepened left 3.3 s of
        // coverage behind it.
        //
        // What that cost, measured on `lucent --sim 300` at 1280x720:
        // `in.foamHist` was above 0.05 on **100% of the water** at a mean of
        // 0.344, and `foam = smoothstep(fnoise +- ferode, wetc)` is very nearly
        // a binariser against a noise centred on 0.5 with sigma 0.116 — so a
        // coverage of a third put the threshold's *most discriminating band*
        // over the entire sea and painted half of it in 6x-stretched lace. That
        // is the shredded ribbons and the nested contour rings the human
        // reported; ablating foam gives a clean sea, and dropping this term
        // alone gives a clean sea. It was latent before `d03e6d1` and grey
        // rather than white (rendered at `9b8ce94`: the same shapes, a quarter
        // the contrast), which is why the age fix appeared to cause it.
        //
        // Swept on the same shot, `legible%` from `foam_legibility.py`:
        //
        //     0.22 / 0.33 (what this was)  14.35%   half the sea is lace
        //     0.33 / 0.50 (this)            4.14%   ribbons behind broken crests
        //     0.38 / 0.55                   3.06%
        //     0.45 / 0.62 (`FoamField`'s)   2.51%
        //
        // The top row is the biggest number and the defect; the metric measures
        // area x contrast and cannot tell foam from a lace-textured sea, which
        // is exactly what it was rewarding. 0.33/0.50 is the pair already in
        // this file for "this crest has broken", so the trail now remembers the
        // state the fragment shader paints white, and no constant was invented.
        //
        // **It does NOT match `FOAM_CREST_BREAK` = 0.45 and should not.** That
        // number was swept against an 8 s half-life with advection to land at
        // 4.73% steady-state coverage; this is a 3.3 s window decaying at
        // `FOAM_TRAIL_DECAY` with no advection, so the same coverage wants a
        // lower gate. Two mechanisms, one question, two calibrations.
        if (past.mu_max > WATER_FOAM_BREAK) {
            float tapUp = waterFoamBreakPatch(
                xz - trailWind * tau, push.environment[0].weather.z - tau);
            trail = max(trail, decay * smoothstep(WATER_FOAM_BREAK + tapUp,
                                                  WATER_FOAM_BROKEN + tapUp,
                                                  past.mu_max));
        }
    }
    }
    out.foamHist = trail;
    out.baseXZ = xz;
    out.waveletSlope = waveletAt.slope;
    // Not a falling sheet. The fragment shader branches on this and every
    // surface vertex has to say so.
    out.nappe = float4(0.0, 0.0, 0.0, 0.0);
    return out;
}

/// Attenuation per metre at roughly 650 / 550 / 450 nm — `K_d`, the diffuse
/// coefficient Jerlov water types are defined by. Red is gone in three metres
/// at the default, blue survives fifty, and that spread is why deep water is
/// blue at all.
///
/// **Authored per body since the optics slice, and it was a constant before
/// that.** The value a scene that says nothing gets is the constant this
/// replaced — Pope & Fry absorption plus Morel molecular scattering, which is
/// pure water and the clearest sea that physically exists — so nothing moved.
/// `loom_scene::components::WaterOptics` is the authoring side and carries the
/// Jerlov table of what other values mean.
float3 waterExtinct() { return push.environment[0].waterAttenuation.xyz; }
/// Backscatter. **The turbidity knob, and the only one.** Raising it turns
/// navy toward teal, because what comes back up out of a water column is
/// scattered light, not reflected light.
float3 waterBackscatter() { return push.environment[0].waterBackscatter.xyz; }

/// How far above still water a **significant crest** on this sea stands, in
/// metres — `WATER_CREST_SIGMA · σ`.
///
/// **Zero is glass**, and every caller must treat it as such rather than
/// dividing by it: a body with no wave set uploads `σ = 0` and there is then no
/// crest for light to cross.
float waterCrestScale() {
    return WATER_CREST_SIGMA * push.environment[0].waterScale.x;
}

/// What deep water looks like when you see into it rather than off it.
///
/// **Derived, not eyeballed.** This is the standard deep-water remote-sensing
/// reflectance `R∞ = 0.33·b_b/(a + b_b)` — the fraction of downwelling light a
/// semi-infinite column scatters back up — evaluated from the two coefficient
/// sets above. It comes out (0.0003, 0.0042, 0.0391): darker than the hand-
/// picked value it replaces in red and green, similar in blue, so the colour
/// *range* widens and the navy goes nearly black.
///
/// **The comment this replaces said most of what a viewer reads as the colour
/// of the sea is the sky bouncing off it. That is a well-known misconception
/// and it was costing us the whole look.** Deep water is blue on a fully
/// overcast day and blue looking straight down where nothing is being
/// reflected, because light enters it, scatters, and comes back out. Measured
/// on the old constant: darkening the sky to an overcast moved our sea's R/B
/// ratio to 0.95 — neutral grey, polished pewter — against 0.62 for a
/// photograph of a real storm sea. A mirror over a void has no colour of its
/// own, and that is what this was.
float3 waterDeep() {
    float3 bb = waterBackscatter();
    return 0.33 * bb / (waterExtinct() + bb);
}
static const float3 WATER_SHALLOW = float3(0.020, 0.070, 0.075);
/// Depth over which the tint reaches its deep value, in metres.
static const float WATER_TINT_DEPTH = 5.0;
/// **Not zero.** A mirror-smooth sea gives a specular lobe the size of a
/// pixel, which is the classic water shimmer; this is the smallest roughness
/// that still reads as sun glitter rather than as a hard white dot.
static const float WATER_ROUGHNESS = 0.08;
/// Water's refractive index against air's 1.0.
///
/// It is here for one number: `asin(1/1.333)` is 48.6°, the critical angle, and
/// that cone is Snell's window.
/// How rough the surface is for the purpose of capping grazing reflectance.
///
/// Not `WATER_ROUGHNESS`, which describes the specular lobe of the sun on a
/// nearly-mirror surface. This is the aggregate of everything finer than the
/// mesh — the capillary ripples above, and the ones below even those — and it
/// is what decides how much of the sky a grazing view really returns.
static const float WATER_CLARITY_ROUGH = 0.22;

static const float WATER_IOR = 1.333;

/// How much sky the traced reflection is diluted with. See the call site.
///
/// Deliberately near zero: `tracedEnvironment` uses its roughness twice — once
/// to widen the lobe direction and once to `lerp` the traced answer back to the
/// miss colour, reaching it exactly at 1.0. A sea's broadening comes from the
/// capillary ripples perturbing the normal per pixel, which is geometry rather
/// than a blend, so paying for it twice would wash the reflection out.
static const float WATER_REFLECT_ROUGH = 0.06;

/// The same, for the cinematic tier's marched free surface.
///
/// **Nine times rougher, and it is the roughness the extraction removed rather
/// than a taste knob.** `fluid_surface::march` samples a 10 cm lattice and then
/// runs `SMOOTH_PASSES` Jacobi sweeps over the fraction field and
/// `NORMAL_PASSES` more over the copy the normal is differentiated from — so
/// the mesh that reaches this shader is, by construction, smooth below about
/// 30 cm. Real water is never smooth at that scale, and the reflection is the
/// one term that notices: neighbouring pixels send their single mirror ray to
/// the *same* triangle of the *same* box, so a whole region agrees on one
/// flat-shaded colour bounded by that box's silhouette. On `plough_cinematic`,
/// from a low camera, that draws the ramp, the back kerb, the end kerb, the
/// hull and the bed as **hard-edged dark polygons lying over the pool** —
/// which is what a human reported as "a reflection at a weird angle", and it
/// is geometrically correct. What is wrong is that a single ray is a good
/// sample of a mirror and this surface is not one.
///
/// The analytic sea never showed it because its Gerstner sum plus the
/// capillary detail decorrelates neighbouring rays; the marched surface has
/// neither, and the wind-driven capillary term is measured at 19/255 of normal
/// perturbation here — far too little to break the agreement.
///
/// Swept at a low camera on `plough_cinematic --sim 3000` and `--sim 6000`,
/// which is where the human found it — from above, `fresnel` is about 0.1 and
/// the term is invisible whatever it says:
///
///     0.06 (the analytic sea's value)   hard-edged dark polygons, unmistakable
///     0.25                              softened, the straight edges still read
///     0.40 / 0.55 / 0.70                gone; a coherent hazy sheet
///
/// **The top three differ by at most 17/255 in the worst channel**, so this is
/// a plateau rather than a knee and 0.55 sits in the middle of it. What
/// remains at every value is a *soft* dark region where the pool genuinely
/// reflects the ramp — that is the reflection working, and deleting it would
/// be the sky-only term ADR 0019 replaced.
///
/// `tracedEnvironment` blends its traced answer toward the miss colour by
/// exactly this number, so what this buys is **dilution with sky, not a lobe
/// width**. It costs nothing: the same one ray, a different blend weight.
///
/// **The lobe width it is not now exists beside it** — `WATER_CINEMATIC_LOBE`
/// jitters the ray per pixel and a quad share averages four of them. The two
/// are not alternatives: measured with the jitter in place, dropping this back
/// toward the analytic sea's 0.06 brings the flat patches back *and* costs an
/// order of magnitude in added salt. The dilution is the variance reduction
/// the jitter is affordable on top of, so it stays at 0.55.
///
/// **Scoped by `WaterOutput::nappe.x`**, which `fluidVertexMain` sets to -1.0
/// and every other water vertex leaves at 0.0 — the same lane, and the same
/// trick, the falling sheet already uses at `+1.0`. Nothing outside the
/// cinematic tier moves: `ocean`, `shore`, `river`, `whitecaps` and
/// `lanternhead` render bit for bit as before.
static const float WATER_CINEMATIC_REFLECT_ROUGH = 0.55;

/// The half-angle of the cinematic surface's reflection lobe, as a tangent.
///
/// **This is the wider lobe the constant above says it is not.** That one
/// dilutes the traced answer toward the sky, which softens the *colour* of a
/// flat patch and leaves its shape exactly where it was: neighbouring pixels
/// still fire one ray each at the same triangle of the same box, so the region
/// still agrees and its boundary is still that box's silhouette. This
/// perturbs the ray itself, per pixel, from a screen-locked dither — the
/// construction ADR 0019 licensed for AO and the soft shadows and for the same
/// reason: the pattern is a function of the pixel's integer coordinate, so it
/// sits still while the world moves under it and a re-render of the same tick
/// is the same bits.
///
/// 4 degrees, measured. Below about 2 the patch interiors are still flat and
/// the straight edges read; above about 8 nothing further happens except haze.
/// Salt is nearly flat across that range once the quad share is in, so the
/// picture picks the number rather than the metric.
///
/// It is also roughly right physically, which is a check rather than the
/// derivation: Cox-Munk gives a calm sea about 2.2 degrees of per-axis RMS
/// slope, so this sits a little under real water.
static const float WATER_CINEMATIC_LOBE = 0.0699;  // tan(4 degrees)

/// Ray origin offset above the surface, in metres.
///
/// Two centimetres rather than the millimetre the opaque path uses: the water
/// mesh is displaced in the vertex shader from a wave sum, so a fragment's
/// interpolated position sits off the true surface by up to half a cell's
/// worth of curvature, and the cell is 0.5 m in the near ring.
static const float WATER_REFLECT_BIAS = 0.02;
/// How much the traced reflection is blurred toward the sky at the range where
/// the capillary ripples have faded out. Tuned on added salt; see the table.
static const float WATER_REFLECT_RANGE = 25.0;

// ---------------------------------------------------------------------------
// Whitecaps.
//
// **Thresholds on the RAW fold, and that is the whole design.** `in.fold` is
// `Σ Q·k·A·sin φ` undivided (see `LoomWaterSample::fold`), so 1.0 is a cusp and
// the validator's `Q·N·k·A ≤ 1` guarantees the sea never reaches it. That makes
// an absolute threshold meaningful: the same number means the same *nearness to
// breaking* on every scene, and a glassy sea gets no foam because it never
// climbs that high. Normalising the fold by `Σ Q·k·A` would destroy exactly
// this — both seas would reach 1.0 at every crest and coverage would become a
// function of wave shape rather than of sea state.
//
// **Two thresholds, not one.** A single cutoff paints a white line along a
// contour of the fold, which reads as drawn rather than as a crest's shape.
// Between them the noise erodes; above the upper one foam wins outright.
//
// Set from σ(fold) = 0.189 on the seven-wave `ocean`: 0.22 is 1.17σ and 0.33
// is 1.75σ. Measured coverage from a top-down render is 4.0–7.0% of surface
// area. **That is a stylised sea's calibration, not a fitted physical law.**
// Monahan's whitecap relation goes as U10^3.52 — a 25x swing between a breeze
// and a storm — while a sum of ≤16 linear Gerstner waves can only move σ by
// about 1.4x under the fold bound, so no fixed threshold can track it. It is
// an order-of-magnitude check only, and 4–7% against Monahan's 3.2% at this
// scene's 14 m/s passes it.
/// Metres of water a ray crosses in a trough **when there is no bed to
/// measure** — the open-sea fallback, and nothing else uses it now that
/// `waterBehind` returns the real column.
static const float WATER_TROUGH_THICK = 4.0;
/// How much thinner the crossing is at a crest than in a trough.
///
/// This is the old `WATER_CREST_THICK = 0.9` written as the ratio it always
/// was — `0.9 / 4.0` — so a bedless sea comes out bit for bit as before while
/// the same crest-thinning applies to a measured column of any depth.
static const float WATER_CREST_THIN = 0.225;
/// Column, in metres, past which thin-water scatter has stopped.
///
/// **A gate, not a strength.** How much light survives the crossing is
/// `exp(-waterExtinct() * thick)` and is not tuned here; this only says where
/// the effect is switched off entirely, so that a deep column cannot glow
/// through the subsurface term. Six metres is `shore`'s own shelf: the shallow band
/// glows, the water past the drop-off does not, and the sea with no bed under
/// it (`column == WATER_NO_BED`) is left to `crest` alone — which is what
/// keeps `ocean` and `squall` byte-identical.
static const float WATER_GLOW_COLUMN = 6.0;
/// What `WaterOutput::depth` clamps to off the height grid. Not a hundred
/// metres of water — "this scene has no voxel volume under it", which is why
/// it gates the column below rather than being fed to it.
static const float WATER_DEPTH_SENTINEL = 100.0;
/// The column `waterBehind` reports when there is nothing behind the water.
/// Large enough that every gate reading it is off, small enough to stay exact
/// in a float.
static const float WATER_NO_BED = 1.0e4;
/// Henyey–Greenstein asymmetry for ocean water. **Derived, not authored.**
///
/// HG's backscatter fraction has a closed form:
///
/// ```text
///   b_b / b = (1 - g)/(2g) · ( (1 + g)/sqrt(1 + g²) − 1 )
/// ```
///
/// Setting that equal to **0.0183** — the Petzold average-particle backscatter
/// fraction that `WaterOptics`'s doc table already builds every `backscatter`
/// row from, `½·b_w + 0.0183·(b − b_w)` — and inverting gives **g = 0.9186**.
/// So this number is a consequence of one already in this codebase rather than
/// a second opinion about the same water, and
/// `loom_render`'s `the_phase_asymmetry_is_petzold_s_backscatter_fraction`
/// re-runs the inversion against this literal so it cannot drift.
///
/// **What it is used for is narrower than the name suggests.** See the long
/// comment at the subsurface term: refraction confines both in-water rays to
/// the Snell cone, so the scattering angle an above-water eye can see is never
/// below 82.8°, and over 82.8–180° this phase function only runs between
/// 0.076 and 0.022 of isotropic — flat to a factor of 1.8 over the angles a
/// real frame produces. It is the right multiplier for the *shallow column* — a real
/// single-scattering path — and it is deliberately **not** applied to the
/// backlit-crest path, which is diffuse transmission and is already carried by
/// `waterExtinct()` being `K_d` rather than the beam coefficient.
static const float WATER_PHASE_G = 0.9186;
/// How far above still water a **significant crest** stands, in units of σ.
///
/// `Hs = 4σ` is the significant wave height — the mean crest-to-trough height
/// of the highest third — so a significant crest reaches `2σ` above the still
/// line. That is the referent, and it is the whole of this number: `lift == 1`
/// means "as high as this sea's own significant crest", whatever sea it is.
///
/// **This is what makes the mask scale-free, which is the defect it fixes.**
/// The gate used to be `crest`, the fold over `WATER_FOLD_REF` — an absolute
/// threshold on *steepness*. On `ocean` that saturates 3.1% of the surface and
/// on `ocean_tropical`'s 0.81 m Pierson–Moskowitz sea it saturates essentially
/// none of it, because a gentle sea is not a shallow storm: it is a *smaller*
/// sea, and its crests are just as much thin sheets with light behind them.
/// Elevation over `2σ` saturates the same 2.3% of a Gaussian surface at every
/// sea state, which is the selectivity the fold gate had on the one sea it was
/// authored against and had nowhere else.
static const float WATER_CREST_SIGMA = 2.0;
/// Fold value treated as "fully a crest" for SHADING ONLY.
///
/// **Never use this to normalise foam coverage.** Foam thresholds absolutely,
/// which is what makes a glassy sea foamless; this is a shading ramp that only
/// needs a sensible span on the seas we author.
static const float WATER_FOLD_REF = 0.35;

/// How shallow water has to get before it is carrying broken foam, in metres.
static const float SHORE_FOAM_DEPTH = 0.28;
/// How far the effective waterline sweeps up and down the beach, in metres of
/// depth, and how fast. Slow: a swash cycle is several seconds.
static const float SWASH_REACH = 0.12;
static const float SWASH_RATE = 0.55;
/// How opaque the swash sheet gets. Well under 1: the sand must show through.
static const float SWASH_OPACITY = 0.62;

/// How thin the water in front of a solid has to get before a waterline band
/// of foam is drawn on it, in metres.
///
/// **This is the one foam term that is not about the sea.** Every other one
/// keys on the water — its fold, its depth over a bed, its wetness. This keys
/// on how little water there is between the surface and *whatever is behind
/// it*, which is the definition of a waterline: the place a solid comes
/// through. `waterBehind` already reconstructs that gap for the refraction
/// offset, so the band costs no new sampling, and it works on a hull, a post,
/// a crate, a wading character and a floating corpse without any of them
/// knowing it exists.
static const float WATER_CONTACT_BAND = 0.35;
/// Its cap. Below `SWASH_OPACITY` because a hull's band is a rim of disturbed
/// water and bubbles, not the couple of centimetres of aerated sheet a swash
/// is — and because both write through `max`, so the thinner one must not be
/// able to hide the thicker.
static const float WATER_CONTACT_OPACITY = 0.55;
/// How far up the water column a solid has to reach before the band draws on
/// it, as a fraction of the still-water depth there.
///
/// **The discriminator between a hull and the seabed.** Both are "the opaque
/// thing behind this water pixel"; only one of them is a waterline. Half the
/// column is the loosest rule that keeps a bed out — the failure it guards is
/// a gentle shelf, where the bed is within `WATER_CONTACT_BAND` of the surface
/// for tens of metres in a row.
static const float WATER_CONTACT_INTRUDE = 0.5;

static const float WATER_FOAM_WET = 0.22;
static const float WATER_FOAM_BREAK = 0.33;
/// Where a broken crest is white along its whole length, on `mu_max` — the far
/// end of the pair `WATER_FOAM_BREAK` opens. It was a bare `0.50` at its one
/// use; it has two now.
static const float WATER_FOAM_BROKEN = 0.50;

/// **The breaking threshold wanders — SEA-REBUILD §4.3.**
///
/// `loom_grass::coverage` and `groundLayerWeight` above both learned this: a
/// cutoff on a smooth quantity — at any threshold, softened by any amount —
/// draws its boundary along a clean curve, and a clean curve is the synthetic
/// tell. Here it is worse than a curve. `in.breakMu` is a **varying**, written
/// per vertex as `surfaceAt.mu_max`, so a constant threshold on it has
/// straight iso-contours *inside every triangle*; the boundary is not a smooth
/// curve, it is the water mesh's own polygons. That is the reported "renders
/// weird": rafts with corners, reading as torn paper.
///
/// **The wander only ever RAISES the pair, and that is not negotiable.** Both
/// numbers move together by the same non-negative amount, so a sea whose
/// `mu_max` never reaches `WATER_FOAM_WET` still foams *nowhere* — which is
/// what every foam gate and every `water@x,z.foam` assertion in this
/// repository rests on. `loom_grass`'s wander only ever subtracts for exactly
/// this reason, and CLAUDE.md records that a symmetric version there was
/// caught by an existing test within a minute. `the_gentle_sea_still_foams_nowhere`
/// is this one's.
///
/// **Two octaves, and the measurement is what says two.** The near field of
/// `ocean_fft_boat --yaw 15 --pitch 55` is entirely inside clipmap level 0 —
/// rendered the Chebyshev bands and the whole lower 80% of the frame is under
/// 32 m — so a triangle edge there is `WATER_CELL` = **0.5 m**, about 8-25
/// pixels at 1000x1000 (`foot` measured at 0.02-0.06 m/px over the same
/// frame). §4.3 asks for a wander "at tens of metres so a crest breaks in
/// patches", and at 28.6 m a cell sees 1.7% of that octave's swing: it moves
/// whole rafts around and cannot bend a single edge. The second octave is
/// sized at the cell instead. One octave cannot do both jobs and the factor
/// between them is about twenty.
///
/// **The amplitudes are sized against the gap, which is 0.11, and against the
/// measured slope of `breakMu`.** Rendering
/// `|grad(in.breakMu)| / foot` in bands over the transition band gives, along
/// the contour (area weighted by the gradient, which is what a contour is),
/// **about 0.35 in `mu_max` units per metre**. So a threshold offset of `A`
/// slides the boundary `A / 0.35` metres, and the cell octave's swing across
/// one 0.5 m cell is what bends the edge rather than translating it.
static const float WATER_FOAM_PATCH_SCALE = 0.035;
/// The patch octave's peak, and it is **rectified** — see
/// `waterFoamBreakPatch`. Half the sea is left at the documented threshold
/// exactly and the other half is progressively suppressed, which is both what
/// "a crest breaks in patches" means and what keeps the coverage. The raw
/// octave was built first and is a uniform tax dressed as patchiness: it raises
/// the threshold *everywhere* by a mean of half its amplitude, and
/// `cargo xtask ablate`'s `whitecaps/water_foam` row fell from **21.422% to
/// 17.166%** against a 15.0% floor for it. Rectified at half this peak the row
/// is **20.205%**, and doubling the peak to this costs 0.075 of a point more
/// (20.130%) because the untouched half is what the row is made of. 0.100 and
/// 0.150 were measured too — 20.014% and 19.897%, with coverage renders
/// indistinguishable from this one — so this is the knee.
static const float WATER_FOAM_PATCH = 0.060;
/// A slice of the noise volume no other term uses. The z axis is hashed like
/// the other two, so a lattice apart is a different field.
static const float WATER_FOAM_PATCH_SLICE = 21.3;
/// Cycles per second the patch field drifts **through** its own z axis.
///
/// §4.3 asks for the wander to drift with the wave. It is done as evolution
/// rather than as translation because the CPU has to reproduce this exactly —
/// `WaterProbe::foam_at` answers `water@x,z.foam` and a wander on one side
/// only is two opinions about the sea — and the CPU probe has the simulated
/// clock and does **not** have the shader's `push.environment[0].wind`. Time
/// on z is the drift both sides can spell. 0.05 is a patch field that turns
/// over in about twenty seconds, against a foam half-life of eight.
static const float WATER_FOAM_PATCH_DRIFT = 0.05;
/// The cell octave: the one that is allowed to bend a polygon edge, so its
/// lattice is sized at the mesh cell rather than at the sea.
///
/// 1.4 cycles/m is a 0.71 m lattice against a 0.5 m cell, so a cell spans most
/// of one period. **Isotropic and unstretched**, for `WATER_FOAM_RAFT_SCALE`'s
/// reason: a stretched threshold field combs the boundary into parallel
/// ribbons.
static const float WATER_FOAM_CELL_SCALE = 1.4;
/// Its amplitude, and **it is not rectified**, unlike the patch octave. This
/// one has to vary *everywhere* — a flat half of it is a flat half of the sea
/// with the straight edges still in it — so it pays its mean, and what it buys
/// is the acceptance test. Judged on the coverage mask alone
/// (`return float4(foam.xxx, 1)` on `ocean_fft_boat --sim 500 --yaw 15
/// --pitch 55`): at 0.035 the flat-topped polygon at the raft's crest is
/// broken but still readable, at this it is gone and the rim is torn, at 0.075
/// the raft is visibly thinner for very little more tearing. Beside it,
/// `cargo xtask ablate`'s row and worst-channel `loom salt` on the reported
/// shot:
///
///     0.035   ablate 20.130   salt 9,072
///     0.055   ablate 19.708   salt 8,922      <- shipped
///     0.075   ablate 19.347   salt 8,812
static const float WATER_FOAM_CELL = 0.055;
/// Its own slice of the noise volume, a lattice clear of every other term's.
static const float WATER_FOAM_CELL_SLICE = 27.9;

/// How much this point's breaking threshold is raised, in `mu_max` units.
/// **Never negative** — see `WATER_FOAM_PATCH`.
///
/// Sampled at the **base** XZ, not at `worldPos.xz`. The displaced position is
/// what the lace and the raft ride, because they are a texture on the drawn
/// surface; this is a threshold the CPU has to agree with at a point, and the
/// CPU knows the base grid and not the displacement it will be drawn at.
///
/// `wLace` is `waterFoamWeights().x`, and the cell octave rides it exactly as
/// `waterFoamRaft` does: at 1.4 cycles/m isotropic it is well under the lace's
/// `0.55 x 6` = 3.3, so that weight bounds it with room to spare. It retires
/// toward **0.5, its own mean**, never toward zero — fading a term to zero
/// instead of to its own mean is the sign error §1 of the band-limit report
/// cost 72% of that gap to, and here zero would mean the far field quietly
/// getting its foam back.
/// The patch half of the wander on its own: §4.3's "tens of metres, so a crest
/// breaks in patches", with none of the cell octave in it.
///
/// **Rectified about the noise's own mean.** `saturate((n - 0.5) * 2)` leaves
/// the lower half of the field at exactly zero — those crests break on the
/// documented threshold, unchanged — and ramps the upper half to the full
/// amplitude. That is the shape "in patches" actually asks for; the raw octave
/// instead raises the threshold everywhere by a mean of `WATER_FOAM_PATCH / 2`,
/// which is a flat tax on foam coverage that happens to vary. The measurement
/// is in `WATER_FOAM_PATCH`.
float waterFoamBreakPatch(float2 xz, float seconds) {
    float n = loom_value_noise(
        float3(xz * WATER_FOAM_PATCH_SCALE,
               WATER_FOAM_PATCH_SLICE + seconds * WATER_FOAM_PATCH_DRIFT));
    return WATER_FOAM_PATCH * saturate((n - 0.5) * 2.0);
}

float waterFoamBreakWander(float2 xz, float seconds, float wLace) {
    float cell = lerp(0.5,
                      loom_value_noise(float3(xz * WATER_FOAM_CELL_SCALE,
                                              WATER_FOAM_CELL_SLICE)),
                      wLace);
    return waterFoamBreakPatch(xz, seconds) + WATER_FOAM_CELL * cell;
}

/// The `LOOM_ABLATE` mask, as `loom_render::ablate` filled it.
///
/// **Zero on every ordinary run**, so a normal frame's `ablated()` call costs a
/// float-to-uint convert, a bitwise AND, a compare and a branch — not "one compare against
/// zero and nothing else". This is a measurement switch: `cargo xtask ablate` renders a
/// scene with and without an effect and fails when the two are too *similar*, because a
/// feature whose removal changes nothing was never drawing.
///
/// **Must equal `water_foam`'s bit in `ABLATIONS` (`crates/loom_render/src/ablate.rs`).**
/// This is the third of three places the bit is recorded — `ABLATIONS`, `xtask`'s own
/// `ABLATE` table, and this constant — and nothing checks that this one agrees with the
/// other two. A mismatch does not fail loudly: `LOOM_ABLATE=water_foam` would ablate
/// whatever effect actually holds this bit in Slang, the frame would still move past the
/// floor, and `cargo xtask ablate` would report `ok` having never measured foam.
static const uint LOOM_ABLATE_WATER_FOAM = 1u;

/// The bit for the FFT cascade — ADR 0076.
///
/// **Must equal `ocean_spectrum`'s bit in `ABLATIONS`
/// (`crates/loom_render/src/ablate.rs`)**, on exactly the warning above: this is the third
/// of the three places that bit is written down and nothing checks the three agree.
///
/// Ablating it zeroes `sea.count` in `waterVertexMain`, which is the value a Gerstner body
/// already carries, so the shader takes its `loom_sample_water` branch — and a spectrum
/// body has no wave list, so that branch draws the flat plane. **A runtime branch and not a
/// constant fold**, which matters here: hard-wiring the flat surface would let the compiler
/// drop the varyings `fragmentMain` no longer reads, and `WARNING-Shader-OutputNotConsumed`
/// is fatal in a debug build (`SEA-REBUILD.md` §7.3).
static const uint LOOM_ABLATE_OCEAN_SPECTRUM = 2u;

/// The bit for the backlit subsurface term on the water surface.
///
/// **Must equal `water_glow`'s bit in `ABLATIONS`
/// (`crates/loom_render/src/ablate.rs`)**, on exactly the warning above: this is the third
/// of the three places that bit is written down and nothing checks the three agree.
///
/// Ablating it zeroes `sss` at its single point of use, so what is measured is the light
/// that came through the water and nothing else — not the subsurface term, not the crest mask,
/// not the crossing. **The registered scene is `ocean_tropical`**, which is the sea this
/// term could not reach until `lift` replaced `crest`: a Beaufort 4 Pierson–Moskowitz sea
/// never crosses `WATER_FOLD_REF`, so the old gate drew literally nothing there and a
/// reference image of it would have recorded the absence and passed for ever.
static const uint LOOM_ABLATE_WATER_GLOW = 4u;

bool ablated(uint bit) {
    return (uint(push.environment[0].viewport.w) & bit) != 0u;
}

// ---------------------------------------------------------------------------
// The foam trail (W2). Read `waterVertexMain` for why it is closed-form.
// ---------------------------------------------------------------------------
/// Seconds between history taps. **Ten taps at 0.33 s, over the three at 1.1 s
/// this replaces — same 3.3 s of trail, sampled where it is still one thing.**
///
/// The step is bounded above by the tap correlation, and ADR 0049 set it by
/// the argument that a crest travels further than a patch is wide in about two
/// seconds. Measured on `whitecaps`' own wave set, the correlation between the
/// coverage now and the coverage at the same drifting point `tau` earlier is
/// 0.91 at 0.1 s, 0.20 at 0.5 s and **crosses zero at 0.70 s** — well inside
/// the shipped 1.1 s step. So the three taps were three independent draws of
/// the same field rather than a trail: against the recurrence they unroll,
/// stepped one fixed tick at a time out to 3.3 s, **three taps recover 33.7%
/// of the foam and miss it entirely on 45.2% of the points that have any**,
/// where ten taps recover 78.6% and miss 8.5%.
///
/// The cost is nine more `loom_sample_water` calls per water vertex. Measured,
/// water pass at 1920x1080 — see the ADR 0049 amendment.
static const float FOAM_TRAIL_STEP = 0.33;
static const int FOAM_TRAIL_TAPS = 10;
/// What a patch is worth one tap later. **`0.1 ^ (FOAM_TRAIL_STEP /
/// WATER_FOAM_AGE_SECONDS)` = 0.9268 — a raft's coverage falls to a tenth over
/// the same ten seconds `foamAge` already ages its albedo across.**
///
/// **It was 0.858 and that was never derived.** Its own doc said so: *"0.858
/// per 0.33 s is 0.60 per 1.1 s, which is exactly the decay the three-tap trail
/// had: the taps got finer, the half-life did not move."* Inherited, not
/// measured — and it is a **1.49 s half-life** against Koepke's ten seconds,
/// five times too fast, on a shader that names ten seconds twice
/// (`WATER_FOAM_OLD_ALBEDO`, `WATER_FOAM_AGE_SECONDS`).
///
/// Koepke has whitecap reflectance falling 55% to 3–10% over about ten
/// seconds, so the survival factor over the window is somewhere in
/// 0.055 to 0.18; the geometric middle is a tenth, which is the number above.
/// The bracket is 0.909 to 0.945 and this sits in it.
///
/// **The old value was load-bearing for the wrong reason, and only the trail's
/// old threshold made it look right.** A trail seeded from `WATER_FOAM_WET`
/// covered the whole sea, and a fast decay was what kept that from being solid
/// white — it dragged the coverage down into the *middle* of the erosion
/// noise's distribution instead, which is the band where a near-binary
/// threshold shreds. With the trail seeded from breaking (see the loop) the
/// area is small, so the decay's job goes back to being what its name says:
/// draining a raft. Measured on `whitecaps --sim 400`, `legible%`:
///
///     0.858 (inherited)  6.17%      0.940   10.97%
///     0.900              8.09%      0.9268  10.02%   <- this
///                                   0.970   12.65%
///
/// **Zero reproduces the pre-W2 picture exactly**, which is the stub the
/// golden gate is measured against: with no decay every tap is worth nothing
/// and `wetc` falls back to the instantaneous coverage.
///
/// **The window is still 3.3 s and this makes its far end visible**: the
/// oldest tap is now worth 0.477 rather than 0.216, so the trail ends at a
/// contour rather than fading out through it. That contour is eroded by the
/// same noise as everything else and I could not find it in a still, but it is
/// the thing to look at first if a trail ever reads as having a hem. The cure
/// is more taps, which is `loom_sample_water` per vertex per tap.
static const float FOAM_TRAIL_DECAY = 0.9268;
/// Fraction of the wind speed foam is carried at. Surface drift is a few
/// percent of the wind over open water, and this is deliberately slower than
/// the *pattern* drift the fragment shader applies to the noise: the noise
/// slides so a patch is not nailed to the world, this moves where the patch
/// was born.
static const float FOAM_TRAIL_DRIFT = 0.02;
/// Stretch of the noise domain **across the breaking direction**. This is what
/// makes streaks — foam pulled out along the crest that made it rather than
/// dropped in round blobs, which is the most recognisable thing about a real
/// whitecap field.
///
/// **It used to lie across the global wind** (`WATER_FOAM_STRETCH`, deleted by
/// ADR 0053 §7). One wind direction is right only for a pure wind sea: a swell
/// crossing the wind, a shore turning it, or a wake had their foam combed the
/// wrong way, and the error was invisible on `whitecaps` precisely because
/// that scene authors every wave within 20° of the wind. `in.breakDir` is the
/// eigenvector of the local compression, so a patch is streaked along the crest
/// it came off.
static const float WATER_FOAM_STREAK = 6.0;
/// Not white. Foam is packed bubbles, and bubbles are dielectric films: the
/// brightest sea foam in a photograph sits well under paper white, and pinning
/// it at 1.0 is what makes rendered surf read as painted.
///
/// **Fresh foam.** Whitecap reflectance falls from about 55% to 3–10% over
/// roughly ten seconds as the bubbles drain and burst (Koepke), so foam is two
/// substances a few seconds apart and drawing both with this is why old foam
/// used to read as paint that had not dried.
static const float3 WATER_FOAM_ALBEDO = float3(0.72, 0.76, 0.78);
/// Foam that has been sitting for the trail's whole length. Not grey paint: a
/// thinning raft of bubbles over water, which is why the opacity below drops
/// with it rather than only the albedo.
static const float3 WATER_FOAM_OLD_ALBEDO = float3(0.28, 0.30, 0.32);
/// How much of the surface the oldest foam still replaces. Under 1 because a
/// dispersing raft is holes: the water's own reflection comes back through it,
/// which is the difference between foam ageing and foam dimming.
static const float WATER_FOAM_OLD_OPACITY = 0.55;
/// How long a raft takes to become `WATER_FOAM_OLD_ALBEDO` — Koepke's own
/// "roughly ten seconds", the same source the two constants above are from, so
/// this is a reading of that table rather than a fourth authored number.
///
/// **It is the denominator `foamAge` was missing.** See the derivation at
/// `foamAge`'s use: the trail carries the elapsed time in its decay and the
/// share threw it away.
static const float WATER_FOAM_AGE_SECONDS = 10.0;
/// **Foam is a material, not a tint — SEA-REBUILD §4.4's second half.**
///
/// A raft of packed bubbles is a rough dielectric. It has a specular lobe, and
/// the lobe is nothing like the water's: `WATER_ROUGHNESS` is 0.08, a mirror
/// that makes the sun a point, and drawing foam with a purely Lambert response
/// was what left every whitecap in this shader reading as matte paper. The
/// water's own mirror is already gone under foam — the composite below
/// *replaces* `lit` rather than adding to it — so this is what comes back in
/// its place, and it is broad enough to be a sheen across the whole raft rather
/// than a highlight on part of it.
///
/// `f0` is a water/air film's, the same 0.02 the surface itself uses; foam is
/// bright because it scatters, not because it is metal.
static const float WATER_FOAM_ROUGHNESS = 0.55;
static const float WATER_FOAM_F0 = 0.02;
/// **The lace modulates opacity, and not only at the rim.**
///
/// `foam` is `smoothstep(fnoise +- ferode, wetc)` — the noise enters as the
/// *threshold* a coverage is eroded against, so it can only bite where `wetc`
/// is inside that band. On a spectrum sea `wetc` is not a gradient: `instant`
/// is `smoothstep(WATER_FOAM_WET, WATER_FOAM_BREAK, breakMu)` and a crest
/// crosses those two numbers in a fraction of a cell, so the mask is 0 or 1
/// with a straight mesh-interpolated line between them. **Measured, not
/// argued** — `return float4(foam.xxx, 1)` on `ocean_fft_boat --sim 500
/// --yaw 15 --pitch 55` is a binary stencil of hard-edged polygons with no
/// lace anywhere inside them. That is the reported "renders weird": a raft was
/// a flat opaque slab of one colour with a straight edge, and the whole octave
/// ladder above it was drawing into a band a pixel or two wide.
///
/// So the lace carves the raft *throughout*. A whitecap is entrained air in
/// holes, thinning as it drains, and the fraction it has stopped covering is
/// what this is. Applied to the coverage, so it is opacity **and** albedo —
/// a hole is water showing through, not grey foam.
///
/// **It band-limits on the ladder's own weight.** `waterFoamRaft` is one raw
/// octave and carries no weight of its own, so this term lerps it toward the
/// *mean* it would have had — `WATER_FOAM_HOLE_MEAN` — on `waterFoamWeights().x`,
/// the same number the lace retires by. Toward the mean and never toward zero,
/// for the reason §1 of the band-limit report gives at length: fading a term to
/// zero instead of to its own mean is the sign error that cost 72% of that gap.
/// Zero here would mean the far field's rafts closing up solid, which is the
/// defect this whole constant exists to remove.
///
/// **One number, and it is 1.0 — a hole is a hole.** A fresh/old pair was
/// built first, on the reasoning that a draining raft should be holier than a
/// breaking one, and deleted: `WATER_FOAM_OLD_OPACITY` and
/// `WATER_FOAM_OLD_ALBEDO` already carry ageing, `foamAge` is 0 across every
/// raft in the reported shot (measured — `return float4(foamAge, holes, foam,
/// 1)`), so the pair had nothing to separate, and at anything under 1.0 the
/// raft never actually opens. That last part is the one that matters and it
/// is what the pictures say: at 0.85 the holes are a dimming and the raft's
/// silhouette is still the mesh's straight line, because a 15%-opaque foam
/// pixel is still a foam pixel. At 1.0 the raft is genuinely cut through, and
/// a hole that reaches the rim tears it — which is where the ragged edge in
/// the after image comes from.
static const float WATER_FOAM_HOLES = 1.00;
/// Width of the noise band the holes open through, in raft-density units.
/// Comparable to the slice's own 0.157 standard deviation, so a hole has an
/// edge and a rim rather than being a wash over the whole raft.
static const float WATER_FOAM_HOLE_BAND = 0.25;
/// The mean of `saturate((raft - 0.5) / WATER_FOAM_HOLE_BAND)` over the
/// slice's own distribution, which is what the term flattens to once the
/// ladder has retired. `loom_value_noise` is a trilinear lerp of uniform
/// hashes, so one octave of it has sigma = `sqrt((1/12)(2/3)^3)` = 0.157 (the
/// derivation `WATER_FOAM_ERODE` gives), and with `c = 0.157 / 0.25 = 0.628`
/// this is `c * (phi(0) - phi(1/c)) + P(z > 1/c)` = 0.628*0.2869 + 0.056
/// = 0.236.
static const float WATER_FOAM_HOLE_MEAN = 0.236;
/// The raft texture's own slice of the noise volume. Any value the three
/// octaves above (0.0, 4.1, 7.3) do not use; a lattice apart is enough for
/// `loom_value_noise`, whose z axis is hashed like the other two.
static const float WATER_FOAM_RAFT_SLICE = 11.7;
/// The raft texture's frequency, in cycles per metre, and it is **isotropic** —
/// the one number here that was chosen against the obvious guess.
///
/// The lace is stretched `WATER_FOAM_STREAK = 6` across its own frame, and the
/// raft was built stretched with it. It is wrong: at 6, and at 2.5, the holes
/// come out as parallel ribbons and a breaking crest reads as braided rope
/// rather than as churn. `whitecaps --sim 400` at 1000x1000, worst-channel
/// `loom salt`: stretched 2.5 scores 3,127 and isotropic scores 3,081; the
/// reported `ocean_fft_boat` shot scores 10,064 stretched and **9,530**
/// isotropic. Both the picture and the number pick the same one.
///
/// The lace wants the stretch because the lace is what lays a patch along the
/// crest that made it; the bubbles inside the patch have no such axis. The
/// frame is still the breaking frame (`waterFoamRaft` samples in `wd`), so the
/// raft rotates with the crest — an isotropic pattern in a rotating frame,
/// which costs nothing and keeps ADR 0053 §7 true of every term here.
///
/// **1.1 is the knee.** 1.8 scores 10,009 and 2.6 scores 10,953 on that shot —
/// past this the raft is finer than the ladder's own bound and starts
/// generating exactly the stipple the ladder exists to stop. At 1.1 the peak
/// frequency is a third of the lace's `0.55 * 6` = 3.3, so
/// `waterFoamWeights().x` bounds it with room to spare.
static const float WATER_FOAM_RAFT_SCALE = 1.1;
// -- The foam's octave ladder, band-limited — SEA-REBUILD §4.4 ---------------
//
// **This replaces `WATER_FOAM_FINE_RANGE = 40.0`**, which retired the second
// octave past a fixed distance in metres. That constant had the mechanism right
// and was wrong in three ways this generalises rather than tunes. Every number
// below is worst-channel `loom salt` on
// `ocean_fft_boat --sim 500 --yaw 15 --pitch 55` at 1000x1000, where the shot
// scored **29,462** before this change against a **5,789** floor — what
// `LOOM_ABLATE=water_foam` scores on the same frame, so the whole lace was
// costing 23,673 of it.
//
// **It faded the octave to zero rather than to its own mean.** `loom_value_noise`
// is on [0,1), so dropping a 0.35-amplitude octave takes 0.175 off `fnoise` —
// and `fnoise` is the *threshold* the coverage is eroded against
// (`smoothstep(fnoise - 0.15, fnoise + 0.15, wetc)`), not a colour. A lower
// threshold is more foam, so the far field grew a wash of extra whitecap whose
// every edge is sub-pixel. Centring that fade on 0.5 and changing nothing else
// scores **12,260**: over half the stipple was foam that existed only because
// retiring an octave had moved the mean.
//
// **The first octave never got the treatment at all**, and it is the larger of
// the two. Its cross-streak lattice period is 30 cm, which is under a pixel long
// before the horizon at any downward camera. Flattening it to its mean as well
// scores **7,487** — within noise of the 7,472 that retiring every octave
// scores, so the first octave is where the remaining lace was.
//
// **And a distance in metres is the wrong instrument**, for the reason
// `FIRE_OCTAVE_STEP` above already gives at length: it is blind to resolution
// and blind to grazing angle, and a downward camera over water is nothing but
// grazing angle. The bound is Nyquist in SCREEN space, from a real derivative.
//
// So every octave rolls off on one rule, and a coarser rung comes in under the
// first as it goes, so structure survives at whatever frequency the range can
// carry instead of the surface going to a flat wash. Judged on `loom salt`,
// which can see stipple — not on `flicker`, which cannot tell "wider" from
// "less stable".

/// Lattice scales of the ladder, in cycles per metre ALONG the streak. Across it
/// they are `WATER_FOAM_STREAK` times finer, and that short axis is the one that
/// goes under a pixel first, so it is the axis the band limit is taken on.
///
/// The coarse rung is two octaves under the lace — `0.55 / 4` — so it survives
/// four times the footprint the lace does, which is what keeps a whitecap from
/// being a flat blob for the whole band between the lace dying and this dying.
static const float WATER_FOAM_COARSE = 0.1375;
static const float WATER_FOAM_LACE = 0.55;
static const float WATER_FOAM_FINE = 2.3;
/// Cycles per pixel an octave is whole below and gone above, the same
/// instrument `FIRE_NYQUIST_LOW`/`HIGH` use and the same units.
///
/// **Half the fire shader's numbers, and that is not caution.** Fire integrates
/// its noise along a ray, so 0.5 cycles per pixel — Nyquist itself — is the
/// honest limit there. This noise is *thresholded*: `smoothstep(fnoise ± 0.15,
/// wetc)` is very nearly a binariser, and binarising a signal puts energy far
/// above its own band, so the lattice is still visible as hatching well before
/// it reaches Nyquist. Measured: retiring at fire's own 0.5/0.25 scores 12,427
/// on the shot above — barely better than centring the fade and not
/// band-limiting at all — and this pair scores 11,059. Half again as
/// conservative (0.1667/0.0833) is worth only 664 more while retiring the lace
/// half as far out, so this is the knee of the curve rather than a hard limit,
/// and the residue past it belongs to the binariser rather than to the lattice.
static const float WATER_FOAM_NYQUIST_LOW = 0.125;
static const float WATER_FOAM_NYQUIST_HIGH = 0.25;

/// How much of one octave this fragment can carry: 1 fully resolved, 0 past the
/// bound. `foot` is **the noise's own frame** per pixel — `foamFrameFoot`,
/// measured once at the top of `waterFragmentMain`.
///
/// **It used to be metres of world per pixel times `WATER_FOAM_STREAK`, and
/// that is a different quantity.** The lace is not sampled in world space: it
/// is sampled at `R(wd) . wp`, a rotation into the local breaking frame (ADR
/// 0053 section 7). The screen derivative of that is `R . dwp + dR . wp`, and
/// the second term — the frame turning under a lever arm `|wp|` — is the one
/// `foot` cannot see. Measured on the buoy shot (`ocean_fft`, eye 2 m above the
/// sea): the true frame footprint is **2x to 50x** `foot * WATER_FOAM_STREAK`
/// over most of the frame, so every octave was declared resolved at up to fifty
/// times the frequency it actually had, and the binariser below turned that
/// into the rectilinear hatching the human reported. `foamFrameFoot` takes the
/// derivative of the coordinate that is actually sampled, which is what the
/// paragraph above always claimed this was.
float foamOctaveWeight(float scale, float foot) {
    float cyclesPerPixel = foot * scale;
    return saturate((WATER_FOAM_NYQUIST_HIGH - cyclesPerPixel)
                  / (WATER_FOAM_NYQUIST_HIGH - WATER_FOAM_NYQUIST_LOW));
}

/// The frame the foam's lace and raft are sampled in, and its screen footprint.
///
/// **This band-limits a coordinate that is genuinely badly behaved; it does not
/// repair it.** `R(wd) . wp` is not translation-invariant: `wp` is an absolute
/// world position, so the `dR . wp` term grows with distance from the world
/// origin, and the same sea drawn 400 m out has a lace 20x finer than the same
/// sea drawn at the origin (measured — offsetting `wp` by 300 m on the buoy shot
/// turns the lace into white noise). No rotation of a scalar noise's domain by a
/// non-gradient direction field can avoid that. What this does is stop the
/// ladder lying about it, so the octaves retire where the coordinate has run
/// away instead of hatching. Anything that wants lace to survive a spinning
/// frame has to stop rotating the domain — flow-aligned taps in a fixed frame,
/// which is a different feature and costs taps.
///
/// **Must be called before any `discard` or early `return`**, for the reason
/// `waterFragmentMain`'s own `foot` gives at length: Slang lowers `discard` to
/// `OpKill`, so a derivative taken after one reads a dead lane.
///
/// `x`/`y` are the frame's forward axis; `z` is the larger of the two screen
/// axes of `R(wd) . worldPos.xz`, in the same units the lattice scales are
/// quoted in — cycles per unit of *this* coordinate, so `foamOctaveWeight` no
/// longer multiplies by `WATER_FOAM_STREAK` (the stretch is already in the
/// coordinate's second component).
///
/// The wind drift the sampler subtracts from `worldPos.xz` is spatially
/// constant, so it contributes nothing to either derivative and is left out.
float3 foamFrameFoot(float2 breakDir, float2 worldXZ) {
    // Renormalised for the reason the sampler gives: `breakDir` is an
    // interpolated per-vertex eigenvector, so it is short wherever two vertices
    // disagree, and a short vector here is a stretched lattice.
    float2 wd = normalize(breakDir + float2(1e-4, 0.0));
    float2 fuv = float2(dot(worldXZ, wd), dot(worldXZ, float2(-wd.y, wd.x)) * WATER_FOAM_STREAK);
    return float3(wd, max(length(ddx(fuv)), length(ddy(fuv))));
}

/// The ladder's three weights at this footprint: lace, coarse, fine.
///
/// Split from the noise because the erosion band needs them too, and because
/// the flow map evaluates the noise twice at one footprint — the weights are a
/// property of the pixel, not of the sample.
float3 waterFoamWeights(float foot) {
    float wLace = foamOctaveWeight(WATER_FOAM_LACE, foot);
    // The coarse rung is the lace's understudy rather than a third layer: it is
    // worth exactly what the lace has lost, so it is silent everywhere the lace
    // is whole and the near field is untouched by its existence.
    float wCoarse = foamOctaveWeight(WATER_FOAM_COARSE, foot) * (1.0 - wLace);
    return float3(wLace, wCoarse, foamOctaveWeight(WATER_FOAM_FINE, foot));
}

/// The foam's lace, in the breaking frame. Split out so the flow map can
/// evaluate it twice; `wd` is the **breaking direction** the streaks lie along
/// (`in.breakDir`, not the wind — ADR 0053 §7) and `w` is `waterFoamWeights`
/// for this fragment's footprint.
///
/// Centred on 0.5, with every octave weighted about that centre, so an octave
/// going away costs contrast and never moves the threshold. Where the fragment
/// carries both of the original two, this is `0.65·n₁ + 0.35·n₂` exactly — the
/// amplitudes the old code drew at zero range. What changed is that it holds
/// out to wherever the pixels last, instead of the second octave beginning to
/// fade at the camera and being gone by 40 m.
float waterFoamNoise(float2 wp, float2 wd, float3 w) {
    float2 fuv = float2(dot(wp, wd), dot(wp, float2(-wd.y, wd.x)) * WATER_FOAM_STREAK);
    return 0.5
         + ((loom_value_noise(float3(fuv * WATER_FOAM_LACE, 0.0)) - 0.5) * w.x
          + (loom_value_noise(float3(fuv * WATER_FOAM_COARSE, 4.1)) - 0.5) * w.y) * 0.65
         + (loom_value_noise(float3(fuv * WATER_FOAM_FINE, 7.3)) - 0.5) * 0.35 * w.z;
}

/// Half-width of the band the coverage is eroded through, given how much of the
/// lace the fragment kept.
///
/// **The other half of band-limiting, and without it the fix loses foam.**
/// `foam = smoothstep(fnoise ± half, wetc)` averaged over a footprint the lace
/// no longer resolves is not that same smoothstep at the noise's mean — it is
/// the noise's own distribution, which is wider. Holding the band at 0.15 while
/// the noise flattens therefore throws coverage away: `cargo xtask ablate`'s
/// `water_foam` row, which is how much of `whitecaps` foam is responsible for
/// at 320x200 — a resolution where almost nothing is resolved — fell from
/// 29.178% to 18.139% against its 15.0 floor with the band held fixed.
///
/// The two numbers are matched slopes, not taste. `loom_value_noise` is a
/// trilinear lerp of uniform hashes, so its standard deviation is
/// `sqrt((1/12)·(2/3)³)` = 0.157, and `0.65·n₁ + 0.35·n₂` has
/// `sqrt(0.65² + 0.35²)·0.157` = 0.116. A `smoothstep` of half-width `B` has
/// the central slope of a normal of `sigma = 0.532·B`, so the flat end wants
/// `B = sqrt(0.116² + 0.080²) / 0.532` = 0.265 to pass the same fraction of
/// pixels the lace used to. Measured: it brings the row back to **22.178%** and
/// costs 21 on `loom salt` — nothing, because a wider transition is a softer
/// edge. It does not reach 29.178% and should not: the rest of that number was
/// the mean shift above painting foam that the noise never justified.
static const float WATER_FOAM_ERODE = 0.15;
static const float WATER_FOAM_ERODE_FLAT = 0.265;
/// How much of the lace's contrast this fragment still carries: 1 where every
/// octave is resolved, 0 where the ladder has retired all of them. The
/// amplitudes are the ones `waterFoamNoise` weights its octaves by, so this is
/// literally "what fraction of the noise's own spread survives here".
float waterFoamLaceAmount(float3 w) {
    return saturate(0.65 * (w.x + w.y) + 0.35 * w.z);
}

float waterFoamErode(float3 w) {
    return lerp(WATER_FOAM_ERODE_FLAT, WATER_FOAM_ERODE, waterFoamLaceAmount(w));
}

/// **The raft's own texture — SEA-REBUILD §4.4's "the lace modulates opacity".**
///
/// A slice of its own rather than a reuse of `waterFoamNoise`, and that is the
/// whole reason this works. That noise is the *threshold* the coverage is
/// eroded against, so foam exists exactly where it is low; carving the raft
/// with it puts every hole where the raft had already ended. Measured: at hole
/// strength 0.90 against `fnoise` the reported shot moved 9,998 -> 9,985 on
/// `loom salt` and the picture did not visibly change, because the two
/// patterns are the same pattern.
///
/// Returned **unweighted**, on [0,1), so the caller can lerp it toward its own
/// mean rather than toward zero as the ladder retires — §1 of the band-limit
/// report: fading a term to zero instead of to its own mean is the sign error
/// that cost 72% of that gap.
///
/// Sampled in the breaking frame (`wd` is `in.breakDir`, not the wind — ADR
/// 0053 §7), so a raft rotates with the crest that entrained its air.
///
/// **A shading bump off this was built and deleted.** Three taps instead of
/// one bought a central-difference slope for the foam's normal; against the
/// bumpless render it moved **0.068% of the reported shot at worst-channel 26,
/// and 0.171% of `whitecaps` at worst 6** — the second is inside the golden
/// gate's own tolerance, so it is a term that cannot be seen paying for two
/// thirds of this function. Same disposal as the grass phase's
/// alpha-to-coverage, for the same reason.
float waterFoamRaft(float2 wp, float2 wd) {
    float2 fuv = float2(dot(wp, wd), dot(wp, float2(-wd.y, wd.x))) * WATER_FOAM_RAFT_SCALE;
    return loom_value_noise(float3(fuv, WATER_FOAM_RAFT_SLICE));
}

/// Capillary detail on the water surface, below anything the mesh can carry.
///
/// **The mesh has a hard floor and this is under it.** Waves are evaluated per
/// vertex and a wave fades out as its wavelength approaches the cell sampling
/// it, so the shortest one that survives is about 2 m. Real water is covered in
/// ripples a tenth of that, and their absence is most of why a Gerstner sea
/// reads as a smooth sheet: the specular lobe has nothing fine to break it up,
/// so a whole swell face lights as one facet.
///
/// **Noise with analytic gradients, not a second wave set.** A second
/// `LoomWaveSet` in the fragment shader was designed and rejected: it is a
/// mutated 392-byte local array full-screen, and `scene.slang` already deleted
/// that exact pattern from `grassWind` for being "400 bytes nobody here reads".
/// Central differences on `loom_value_noise` at a fixed epsilon cost three taps
/// an octave and no storage.
///
/// **No `ddx`/`ddy` of its own.** The shoreline `discard` is very nearly the
/// first statement of `waterFragmentMain`, so a derivative taken from here is
/// undefined in the quads that straddle the waterline — exactly the pixels
/// `shore` and `river` exist to protect. `WATER_DETAIL_RANGE` below is computed
/// from `length(toEye)` instead, which is an analytic footprint and correct in
/// a killed quad. The foam's band limit does read a real derivative and pays
/// for it by taking it *before* the discard, in uniform control flow, as the
/// entry point's first statement; this term could read the same `foot` and has
/// not, because widening the capillary detail re-blesses every water reference
/// in the repository for a symptom nobody has reported.
static const float WATER_DETAIL_SCALE = 1.6;

// -- How hard the capillary detail tilts the surface — W11 -------------------
//
// **It used to be one number, `WATER_DETAIL_STRENGTH = 0.30`, applied
// unconditionally to every water surface in every scene.** That is a real
// wind speed — Cox–Munk inverts 0.30 to about 17 m/s, a near gale — so
// `pool.loom`, whose header calls it dead calm and authors `Wind.speed = 0`,
// was drawn covered in chop. The human asked for "the ripples from the wind,
// integrated with what we have"; the integration is that the term which was
// already painting wind texture starts *hearing* the wind, rather than a
// second ripple system growing beside it.
//
// **Presentation only.** Nothing here reaches a force, a script or `loom sim
// --assert` — the force-path interactive surface (ADR 0056's events) is
// untouched by it. See ADR 0052 for why wind forcing was kept off the force
// path; the grid that ADR argued about is now gone, and the argument that wind
// chop is presentation survives it.

/// Cox & Munk (1954), from sun-glitter photographs of the Pacific: the
/// mean-square slope of a wind-roughened surface is `0.003 + 5.12e-3 · U`, with
/// `U` the wind speed in m/s. The detail term is an RMS *slope*, so it is the
/// square root of that — and the two constants are the intercept and the
/// gradient of that line rather than anything anybody tuned.
///
/// The intercept is what a dead calm gets: 0.055, a fifth of the old constant,
/// which is the difference between a millpond and a lake in a breeze. It is
/// deliberately not zero — real water is never a mirror, and a perfectly
/// specular pool reads as glass rather than as water.
static const float WATER_DETAIL_CALM = 0.003;
static const float WATER_DETAIL_COXMUNK = 5.12e-3;
/// Fraction of the wind speed the pattern is carried downwind at.
///
/// **The floor is gone and that is the point.** This was `max(speed, 0.5)`, so
/// a scene at `speed = 0` still had its chop crawling across the surface at
/// 3 cm/s — the single most visible reason a calm pool did not read as calm.
///
/// **Global, while the amplitude above is per pixel**, and the asymmetry is
/// deliberate. Advecting the noise domain by a *spatially varying* velocity
/// makes `xz - drift(xz)·t` fold once `t·∂drift` approaches the gust
/// wavelength — a smear that appears only after minutes of `loom run` and that
/// no still could catch. The pattern is a metre across and the gust field is a
/// hundred, so a per-pixel drift would buy nothing visible for that risk. The
/// gusts still travel: `wind_at` carries its own `t`, so the patches move
/// because the wind field does.
static const float WATER_DETAIL_DRIFT = 0.03;

// -- The interactive wake, made visible — W10 -------------------------------
//
// **Presentation only, all three of these.** The grid is stepped on the CPU and
// is what buoyancy reads (ADR 0046); nothing below reaches a force, a script or
// `loom sim --assert`, and none of it is fed back into `WaterSample::fold`.
// They exist because the wake was *measured* — 34–44% of pixels move on
// `pool.loom` at tick 150 — and could not be *seen*, which is half a feature.
//
// The physics knobs are deliberately untouched. `strength` is measured
// saturated by the relative-velocity coupling that keeps it stable: on
// `pool.loom`, 5.6x the authored value moves the ripple-vs-none mean only
// 7.02 to 11.37 while leaving the sphere at -0.500 m at tick 300 instead of
// +0.140 m [measured]. An invisible physics change bought for a presentation
// problem is the worst trade available here.

// **`RIPPLE_FOAM_SLOPE` and `RIPPLE_FOAM_MAX` are gone from this shader —
// ADR 0053 §7.** They painted the wake's crest white from the ripple grid's
// instantaneous slope, which is foam with no memory: it appeared and vanished
// with the ring rather than being left behind by it. The two numbers survive as
// *deposit* thresholds on the CPU foam field (`loom_water::foam`), where what
// they produce persists and drifts, which is what a wake's foam does.
//
// **`RIPPLE_CALM` and `RIPPLE_CALM_SLOPE` are gone — ADR 0053 §7.** They were
// a standing term: the wake's own slope suppressing the capillary detail
// everywhere the CPU ripple grid reached, which is a mechanism (a wave passing
// under water stretches the surface and calms it) applied at the one place the
// engine happened to have a slope handy. The mechanism now lives inside
// `rainRingsAt`'s envelope, where the ring that earns the calm is the ring
// being calmed.
/// Where the detail stops. `loom_value_noise` is analytic and has no mip chain,
/// so past this the lattice is under a pixel and becomes a twinkle generator
/// rather than detail — the same bound the foam's fine octaves take, for the
/// same reason.
static const float WATER_DETAIL_RANGE = 70.0;
/// The capillary slope at one point, with the wind's global drift and nothing
/// else. Split out so the flow map below can evaluate it twice.
float2 waterDetailSlopeAt(float2 xz, float t, float2 drift) {
    float2 a = (xz - drift * t) * WATER_DETAIL_SCALE;
    // The second octave runs crosswise and faster, so the two do not travel as
    // one sheet — which is what a single scrolling layer always looks like.
    float2 b = (xz + float2(-drift.y, drift.x) * t * 0.7) * (WATER_DETAIL_SCALE * 2.9);
    const float e = 0.5;
    float2 g;
    g.x = (loom_value_noise(float3(a.x + e, 0.0, a.y))
         - loom_value_noise(float3(a.x - e, 0.0, a.y))) / (2.0 * e);
    g.y = (loom_value_noise(float3(a.x, 0.0, a.y + e))
         - loom_value_noise(float3(a.x, 0.0, a.y - e))) / (2.0 * e);
    g.x += (loom_value_noise(float3(b.x + e, 3.1, b.y))
          - loom_value_noise(float3(b.x - e, 3.1, b.y))) / (2.0 * e) * 0.5;
    g.y += (loom_value_noise(float3(b.x, 3.1, b.y + e))
          - loom_value_noise(float3(b.x, 3.1, b.y - e))) / (2.0 * e) * 0.5;
    return g;
}

/// The capillary slope, carried downstream by the river's own current.
///
/// **The branch is not an optimisation.** Still water must render bit for bit
/// as it did, and `wA·g + (1-wA)·g` is not `g` in floats — it is a rounding
/// difference in every water pixel of every scene in the repository, which is
/// exactly the drift only `cargo xtask repeat` can see. The wake's own
/// `pool.count > 0` guard is here for the same reason and was measured.
///
/// A gradient is linear, so blending the two phases' slopes is the same thing
/// as blending the two phases' surfaces and differencing — no approximation is
/// being made by doing it in this order, only two taps saved.
float2 waterDetailSlope(float2 xz, float t, float2 drift, float2 flow) {
    if (dot(flow, flow) <= 0.0) { return waterDetailSlopeAt(xz, t, drift); }
    LoomFlowPhase p = waterFlowPhase(xz, flow, t);
    return waterDetailSlopeAt(p.uvA, t, drift) * p.wA
         + waterDetailSlopeAt(p.uvB, t, drift) * (1.0 - p.wA);
}

// -- Rain rings on open water: the dispersive lattice ------------------------
//
// **A raindrop does not leave one ring, and that is the whole feature.** Every
// ring lattice in this engine before now — `rippleNormal`, which does the same
// job for wet *ground* — expands one crest at one speed, so an impact reads as
// a single hoop and a pool full of them reads as a pattern. Reference image 59
// is a photograph of the thing that is actually happening: several concentric
// bands per impact, crests appearing at the centre and racing outward through
// a slower-moving envelope.
//
// That is deep-water dispersion, and it costs one extra `sin`. Gravity waves
// have `c_p = 2 c_g`: the packet's envelope travels at the group speed and the
// individual crests travel through it at twice that, appearing at the trailing
// edge and dying at the leading edge. So the height of one ring is
//
//     h(d) = exp(-((d - r)/W)^2) * sin((d - 2r) * K),    r = c_g * age
//
// — a Gaussian envelope centred at `r` and a carrier whose crests sit at
// `d = 2r + const`. `K = π/W` puts two crests inside the envelope, which is
// what a raindrop ring has.
//
// **Presentation only, and it must stay that way.** This reads the *CPU* rain
// rate out of the environment block and the cloud cover the same shader
// already evaluates — never the GPU drop buffer, which is ADR 0045 clause 2
// and would be a device fact re-entering the picture. The CPU and the GPU
// sample the same rate field independently and agree about the weather without
// sharing a byte, which is exactly the two-plumes arrangement ADR 0047 shipped.
//
// **It is not `rippleNormal`.** That one perturbs wet ground in the main
// fragment shader at a 0.35 m lattice tuned for tarmac; this one runs in the
// water block, feeds the detail suppression below, and has a different scale.
// Unifying them would move three references that cannot show the difference —
// a ring on wet stone is a specular wobble, not a shape — for no visual gain.

/// Metres per lattice cell. One impact per cell per [`RAIN_RING_PERIOD`].
///
/// **0.9, and not the 0.4-0.5 the brief for this slice asked for, because a
/// 3x3 neighbourhood and a 0.45 m cell cannot hold this ring.** A point in cell
/// C only sees centres in C +- 1, and the nearest point of cell C +- 2 is one
/// full cell away - so a 3x3 search captures every feature whose reach is at
/// most one cell, and clips everything beyond it against a straight lattice
/// line. This packet reaches `RAIN_RING_GROUP_SPEED * RAIN_RING_LIFE` plus two
/// envelope widths: 0.84 m. At 0.45 m the far half of every ring would be cut
/// off on a grid; at 0.9 m nothing is. The alternatives were a 5x5 search (25
/// hashes a pixel for the same picture) or a ring half the size, which is not
/// what a raindrop leaves.
static const float RAIN_RING_CELL = 0.9;
/// Seconds between a cell's chances to be hit.
static const float RAIN_RING_PERIOD = 1.6;
/// How long one ring lives, in seconds.
static const float RAIN_RING_LIFE = 1.2;
/// The envelope's speed, m/s. The crests travel at twice this.
///
/// 0.5 takes the packet to 0.6 m in its 1.2 s life, which is a raindrop ring on
/// still water at roughly life size.
static const float RAIN_RING_GROUP_SPEED = 0.5;
/// Half-width of the envelope, metres. `K = π/W` follows from it.
static const float RAIN_RING_WIDTH = 0.12;
/// How far a ring tilts the normal at its steepest.
///
/// **0.08, and the number is the point.** The honest tilt of a ring this size
/// is 0.06–0.12; the unconditional capillary noise it competes with is 0.30,
/// which is why the envelope also *suppresses* that noise below rather than
/// merely adding to it. A ring that only adds is a change in the texture of a
/// shimmering field rather than a shape — the same finding `RIPPLE_CALM` was
/// written for, now applied where it is earned.
static const float RAIN_RING_TILT = 0.08;

/// Every ring passing through this point, as a radial slope, plus how deep
/// inside an envelope the point is.
///
/// Returns the xz slope to subtract from the normal. `envelope` comes back in
/// `[0, 1]` and is what the capillary detail is suppressed by.
///
/// `gate` is `rate x cover x shelter` already multiplied together by the
/// caller — zero short-circuits the whole function, which is what makes a dry
/// scene pay one compare.
float2 rainRingsAt(float2 xz, float t, float gate, out float envelope) {
    envelope = 0.0;
    float2 slope = float2(0.0, 0.0);
    if (gate <= 0.0) {
        return slope;
    }

    const float SCALE = 1.0 / 16777216.0;
    const float K = 3.14159265 / RAIN_RING_WIDTH;
    float2 p = xz / RAIN_RING_CELL;
    int2 base = int2(floor(p));

    // 3x3, because a ring reaches further than its own cell. The bound is what
    // keeps this to nine hashes rather than a search.
    for (int j = -1; j <= 1; ++j) {
        for (int i = -1; i <= 1; ++i) {
            int2 cell = base + int2(i, j);
            uint seed = loom_hash(loom_hash(uint(cell.x)) + uint(cell.y));
            // Phased per cell, so neighbouring impacts are not simultaneous.
            float phase = float(seed >> 8) * SCALE;
            float s = t / RAIN_RING_PERIOD + phase;
            float cycle = floor(s);
            float age = (s - cycle) * RAIN_RING_PERIOD;
            if (age >= RAIN_RING_LIFE) {
                continue;
            }
            // **Rehashed per impact**, so the next drop in this cell lands
            // somewhere else. Hashing the centre from the cell alone makes
            // every cell a tap dripping on one spot and the lattice becomes
            // visible in two seconds — `rippleNormal` learned that first.
            uint h = loom_hash(seed + uint(int(cycle)));
            uint h2 = loom_hash(h);
            uint h3 = loom_hash(h2);
            // Light rain rings in fewer places as well as more faintly, the
            // same per-cell thinning the drops themselves take.
            if (float(h3 >> 8) * SCALE >= gate) {
                continue;
            }

            float2 centre = (float2(cell) + float2(float(h >> 8) * SCALE,
                                                   float(h2 >> 8) * SCALE))
                          * RAIN_RING_CELL;
            float2 offset = xz - centre;
            float d = length(offset);
            if (d < 1e-4) {
                continue;
            }
            float r = RAIN_RING_GROUP_SPEED * age;
            float u = (d - r) / RAIN_RING_WIDTH;
            if (abs(u) > 2.5) {
                continue;
            }
            // The packet, and the derivative of it. `E' S + E S'`, exactly —
            // the slope IS what a normal perturbation wants, and differencing
            // a height would need two evaluations of the same nine cells.
            float e = exp(-u * u);
            float carrier = (d - 2.0 * r) * K;
            float radial = e * (K * cos(carrier) - (2.0 * u / RAIN_RING_WIDTH) * sin(carrier));
            // Fade the whole packet out over its life, so a ring dies rather
            // than being cut off at `RAIN_RING_LIFE`.
            float fade = 1.0 - age / RAIN_RING_LIFE;
            slope += (offset / d) * (radial / K) * fade;
            envelope = max(envelope, e * fade);
        }
    }
    return slope * RAIN_RING_TILT;
}

/// Colour and depth of everything drawn BEFORE the water. Set 3.
///
/// The forward pass ends after the grass and resolves into these; the water
/// block declares them readable and samples them here. Single-sample, because
/// a multisampled image cannot be sampled per-pixel without a manual
/// per-sample fetch — the resolve IS the mechanism.
[[vk::binding(0, 3)]] Sampler2D opaqueColor;
[[vk::binding(1, 3)]] Sampler2D opaqueDepth;

/// How far light travels through water between the surface and whatever is
/// behind it, and how much survives the trip.
///
/// Returns `T` — the fraction of each wavelength that makes it — and writes
/// the background colour it found. `T = 0` means "nothing behind this pixel",
/// which is the open-ocean answer and costs the caller nothing.
///
/// **The guard is on the raw depth sample, not on the subtracted distance.**
/// The far plane is 1000 m while the water skirt reaches 50 km — the vertex
/// shader deliberately drags it inside the clipper — so past the far plane a
/// distance subtraction goes NEGATIVE, and a `max(.., 0)` on it would hand
/// back `T = 1`: the sea beyond a kilometre rendered as a transparent hole
/// onto the sky. Sky writes no depth, so one test covers the horizon skirt,
/// the sentinel scenes and any future far-plane change at once.
/// Metres of water past which the caustic web is washed out.
static const float CAUSTIC_DEPTH = 4.5;
static const float CAUSTIC_SCALE = 0.85;
/// How tightly the two fields have to agree to count as a focus. Higher is
/// thinner, brighter filaments.
static const float CAUSTIC_SHARP = 9.0;
static const float CAUSTIC_POWER = 3.0;
static const float CAUSTIC_STRENGTH = 0.85;

/// The caustic web at a point on the bed, as a multiplier on its lit colour.
///
/// Extracted because it is wanted from two places: the water shader looking
/// down through the surface, and the terrain shader when the terrain itself is
/// underwater. A seabed with caustics seen from above and none seen from below
/// is the same surface disagreeing with itself.
/// The web itself at one point, before the depth and sun fades. Split out so
/// the flow map can evaluate it twice.
float causticWebAt(float2 cuv, float2 cdrift) {
    float c1 = loom_value_noise(float3(cuv + cdrift, 0.0));
    float c2 = loom_value_noise(float3(cuv * 1.31 - cdrift * 0.83, 5.7));
    return pow(saturate(1.0 - abs(c1 - c2) * CAUSTIC_SHARP), CAUSTIC_POWER);
}

float causticWeb(float3 bedPos, float depth) {
    float2 cdrift = float2(0.07, 0.05) * push.environment[0].weather.z;
    // **The bed's own position, advected by the current above it — W8.** The
    // filaments are the surface's focus lines projected down, so they travel
    // with the surface; a static web under a moving river is the tell that the
    // water is a texture rather than a fluid.
    //
    // `flowField()` rather than an argument, because this is called from two
    // places — the water looking down and the terrain shader when the terrain
    // is itself underwater — and a seabed with a crawling web seen from above
    // and a still one seen from below is the same surface disagreeing with
    // itself. The branch keeps every still-water scene exact.
    float2 flow = loom_flow_at(flowField(), bedPos.xz);
    float web;
    if (dot(flow, flow) <= 0.0) {
        web = causticWebAt(bedPos.xz * CAUSTIC_SCALE, cdrift);
    } else {
        LoomFlowPhase p = waterFlowPhase(bedPos.xz, flow, push.environment[0].weather.z);
        web = causticWebAt(p.uvA * CAUSTIC_SCALE, cdrift) * p.wA
            + causticWebAt(p.uvB * CAUSTIC_SCALE, cdrift) * (1.0 - p.wA);
    }
    // Strongest just under the surface and gone by a few metres — a ripple's
    // focal length is short and a deep column washes the web out entirely.
    // Gone at a low sun too, since a grazing ray never focuses.
    float fade = saturate(1.0 - depth / CAUSTIC_DEPTH)
               * saturate(sunDirection().y * 1.6);
    return web * CAUSTIC_STRENGTH * fade;
}

/// How close to the frame border the offset is given up, in UV.
///
/// Outside the image there is nothing to bend into — the opaque pass drew what
/// the camera saw and no more — so a lookup that leaves the frame clamps and
/// smears the border pixel along the whole edge. Fading back to the straight
/// UV over the last 6% costs the offset where it is least visible and where
/// there is no honest answer available anyway.
static const float WATER_REFRACT_EDGE = 0.06;
/// Metres the offset lookup must land BEHIND the surface before it is trusted.
///
/// **Attenuated, never rejected.** A hard switch traces the silhouette of
/// anything crossing the water as an outline that breathes with the waves, and
/// the depth target is a `SAMPLE_ZERO` resolve, so there is no coverage
/// information to soften it with. Fading the offset out instead falls back to
/// the un-offset UV, which can never be wrong: this fragment *is* water there.
static const float WATER_REFRACT_ACCEPT = 0.35;

/// Where a ray leaving the surface downward lands, in screen UV.
///
/// **Marched along the refracted ray by the depth of the water, never by the
/// distance to the background.** The background was found along the *air* ray,
/// which on a grazing view is bent tens of degrees away from this one — on
/// `shore` that is 35 m against a column of 6 — so marching the air distance
/// puts the probe deep inside the seabed and the offset comes out enormous.
/// `drop / -refracted.y` is the distance along this ray that descends the
/// column, which is the only length that means anything here.
float2 waterProbeUv(float3 surfacePos, float3 refracted, float drop, float2 straightUv) {
    // 0.05 is a 3-degree floor on the descent. Air->water can only bend a ray
    // toward the vertical, so this is unreachable in practice and is here so a
    // degenerate normal cannot divide by zero.
    float3 probe = surfacePos + refracted * (drop / max(-refracted.y, 0.05));
    float4 clip = mul(push.objects[push.objectOffset].mvp, float4(probe, 1.0));
    if (clip.w <= 0.0) {
        return straightUv;
    }
    float2 uv = (clip.xy / clip.w) * 0.5 + 0.5;
    float2 edge = min(uv, 1.0 - uv);
    return lerp(straightUv, uv, saturate(min(edge.x, edge.y) / WATER_REFRACT_EDGE));
}

/// Secant of the path a downward ray takes through water, from the cosine of
/// its angle to the vertical **in air**.
///
/// Snell, written as the only thing the absorption needs. It replaces a
/// hand-picked `WATER_DOWN_SEC` and a `1 / max(sunDirection().y, 0.15)`, and
/// it is bounded by construction: `1/cos(asin(1/1.333))` is **1.5124** at
/// grazing incidence, so a sun on the horizon lights a 6 m bed through 9 m of
/// water and not through infinity. That bound is why the old clamp existed and
/// why it is no longer needed — the physics does the clamping.
float loomWaterSecant(float cosAir) {
    float sinT2 = (1.0 - cosAir * cosAir) / (WATER_IOR * WATER_IOR);
    return 1.0 / sqrt(max(1.0 - sinT2, 1e-4));
}

/// Unproject a depth sample back to where it was drawn.
float3 waterUnproject(float2 uv, float d) {
    float4 world = mul(push.invViewProj, float4(uv * 2.0 - 1.0, d, 1.0));
    return world.xyz / world.w;
}

float3 waterBehind(
    float2 uv,
    float3 surfacePos,
    float3 eye,
    float bedDepth,
    float3 refracted,
    out float3 behind,
    /// Metres of water the light crossed, both legs — `WATER_NO_BED` when
    /// there is nothing behind the surface. The subsurface term reads it, so
    /// that how thin the water is is measured once rather than authored twice.
    out float column,
    /// Metres of water between the surface and whatever OPAQUE thing is behind
    /// it — `WATER_NO_BED` when there is nothing behind it at all.
    ///
    /// **Not `column`, and the difference is the whole of the waterline band.**
    /// `column` is an optical path through *a bed the height grid knows about*,
    /// and is deliberately left at the sentinel on every scene with no voxel
    /// volume so that `ocean`'s submerged posts do not tell the subsurface term
    /// they are standing in a hundred metres of water. That is the right rule for an
    /// absorption path and the wrong one for a waterline: it makes `column`
    /// unusable on the open sea, which is where every boat is. This is a
    /// geometric gap to the depth buffer, so it answers there.
    out float gap)
{
    behind = float3(0.0, 0.0, 0.0);
    column = WATER_NO_BED;
    // Only ever written past the `d >= 1.0` early-out below, so a pixel with
    // nothing opaque behind it keeps the sentinel and draws no band. That
    // matters most at the horizon, where the view ray is nearly level and the
    // far plane sits at about eye height: the gap there would be zero and the
    // band would be a white line across the skyline.
    gap = WATER_NO_BED;
    float d = opaqueDepth.SampleLevel(uv, 0).r;
    if (d >= 1.0) {
        return float3(0.0, 0.0, 0.0);
    }
    // Reconstruct where that pixel is, so the path length is the real one
    // rather than an authored depth. `in.depth` is a still-water bed depth
    // clamped at 100 and is a SENTINEL on any scene with no voxel volume;
    // using it as an optical path is what would paint 27% seabed brown across
    // every open-ocean pixel.
    float3 bg = waterUnproject(uv, d);

    // **The offset: what the surface actually shows, rather than what is
    // directly behind it.** The straight lookup above is the right *column* —
    // it is the water under this pixel — but it is the wrong *pixel*: a tilted
    // surface bends the view ray, which is why a stick in water has a kink in
    // it and why a rippled pool's floor swims. There is no strength knob; the
    // offset weakens with distance for free, because the same world
    // displacement subtends fewer pixels further away, which is exactly what a
    // `normal.xz * strength` fudge cannot do.
    //
    // **One refinement.** The first probe descends by the drop measured under
    // the straight ray; the second uses the drop it actually found. Two steps
    // is where a fixed point of this shape stops moving visibly — the
    // remaining error is the bed's slope across the offset, which is metres of
    // sand, not a silhouette.
    {
        float2 probeUv = waterProbeUv(surfacePos, refracted,
                                      max(surfacePos.y - bg.y, 0.0), uv);
        float pd = opaqueDepth.SampleLevel(probeUv, 0).r;
        if (pd < 1.0) {
            float3 probeBg = waterUnproject(probeUv, pd);
            probeUv = waterProbeUv(surfacePos, refracted,
                                   max(surfacePos.y - probeBg.y, 0.0), uv);
            pd = opaqueDepth.SampleLevel(probeUv, 0).r;
            if (pd < 1.0) {
                probeBg = waterUnproject(probeUv, pd);
                // How far behind the surface the offset landed. Something in
                // FRONT of the water — a hull, a post, the near bank — scores
                // zero and the offset dissolves back to the straight lookup
                // rather than smearing an above-water pixel into the sea.
                float accept = smoothstep(0.0, WATER_REFRACT_ACCEPT,
                                          length(probeBg - eye) - length(surfacePos - eye));
                // Colour, caustic anchor and path length all move together:
                // the alternative is a bed shaded from one place and measured
                // from another, which flickers along the accept band.
                uv = lerp(uv, probeUv, accept);
                d = opaqueDepth.SampleLevel(uv, 0).r;
                bg = waterUnproject(uv, d);
            }
        }
    }

    // **Two legs, and both of them are inside the water.** The seabed was
    // shaded IN AIR by the mesh pass, so the light that lit it already crossed
    // the column downward before it ever reflected toward the eye.
    //
    // Both legs are the *refracted* path, not the air one. The view leg
    // descends `drop` metres along the ray this fragment already bent, and the
    // down leg takes Snell on the sun's elevation. On `shore`'s low sun the
    // old `1 / sunDirection().y` was 17.6 m of water against the 8.5 m light
    // actually crosses — the same air-path mistake the probe above refuses to
    // make, in the other direction.
    float drop = max(surfacePos.y - bg.y, 0.0);
    // **The seabed is not a waterline, and this is the line that says so.**
    // The band wants the solid that comes THROUGH the surface. On a beach the
    // nearest solid is the bed itself, over the whole shallow apron — so
    // ungated it washed `beach.loom`'s wet sand milky across 15.3% of the
    // frame, a second opinion about water `shoreFoam` already owns.
    //
    // A hull, a post or a crate sits in the UPPER part of the column; a bed is
    // the bottom of it. Comparing the two is scale-free, which a second depth
    // threshold beside `SHORE_FOAM_DEPTH` would not be: it says the same thing
    // in a 30 cm rock pool and in an ocean, and it needs no number to be kept
    // in step with the swash.
    gap = drop < min(bedDepth, WATER_DEPTH_SENTINEL) * WATER_CONTACT_INTRUDE
        ? drop
        : WATER_NO_BED;
    float viewLeg = drop / max(-refracted.y, 0.05);
    float downLeg = min(bedDepth, WATER_DEPTH_SENTINEL) * loomWaterSecant(sunDirection().y);
    // **Only a bed the height grid actually knows about reports a column.** At
    // the sentinel the depth is not 100 m of water, it is "no voxel volume
    // here" — so `ocean`'s submerged posts must not tell the subsurface term
    // they are standing in a hundred metres. `T` still uses it, and gets the same
    // answer it always did: nothing survives that path either way.
    if (bedDepth < WATER_DEPTH_SENTINEL) {
        column = viewLeg + downLeg;
    }
    behind = opaqueColor.SampleLevel(uv, 0).rgb;

    // **Caustics: the bright web the surface focuses onto the bed.**
    //
    // The surface acts as a lens. Where it is convex it converges sunlight
    // into a bright filament, where concave it spreads it — and because the
    // ripples move, the web crawls. It is the single most recognisable thing
    // about shallow sunlit water and no amount of correct absorption produces
    // it, because absorption only ever removes light evenly.
    //
    // **The minimum of two scrolling noise fields**, which is the standard
    // cheap construction and not an arbitrary one: a caustic web is the set of
    // places where light from *several* directions arrives at once, so an
    // intersection of two fields has the right topology — thin closed cells
    // with bright junctions — where a single field only gives blobs.
    //
    // Sharpened hard, because real caustics are nearly all dark with a few
    // very bright lines. Anchored to the BED's world position, not the
    // surface's: the pattern belongs to the sand, and anchoring it to the
    // surface would make it swim as the eye moves.
    behind *= 1.0 + causticWeb(bg, min(bedDepth, WATER_DEPTH_SENTINEL));
    // **Un-fog it.** The mesh pass already fogged this pixel over eye->bg, and
    // the last line of the caller fogs the composite again over eye->surface.
    // Applied twice that is 18% becoming 33% on `shore`. What is left over is
    // the surface->bg segment, one or two metres, under a percent.
    // Fogging is `lerp(colour, fogColour, f)`, so removing it is
    // `(fogged - fogColour*f) / (1 - f)` — an EXTRAPOLATION away from the fog
    // colour, not a blend toward it. Writing it as a saturated `lerp` toward
    // `fogColor` brightens instead of removing, and since fog colour here is
    // lit sky that reads as a warm wash: red came out 26% high on `shore` and
    // the shallows went neutral.
    //
    // **Clamped at 0.15, which is where the extrapolation stops being
    // information.** A fragment 90% fogged out has a tenth of its own colour
    // left in it and dividing by `1 - f` multiplies whatever survived — and
    // now the offset feeds this UVs from a neighbourhood, so one dark texel
    // beside a bright one becomes a bright texel in a place it was never
    // drawn. At `1e-3` the multiplier reached a thousand. Past 85% fog the
    // surface itself is fogged out too, so the term it is feeding is invisible
    // either way.
    float3 bgFog = fogAmount(eye, bg);
    behind = max((behind - fogColor(normalize(bg - eye)) * bgFog) / max(1.0 - bgFog, 0.15),
                 float3(0.0, 0.0, 0.0));

    return exp(-waterExtinct() * (viewLeg + downLeg));
}

// ---------------------------------------------------------------------------
// The falling sheet's shading — ADR 0054.
//
// It is drawn INSIDE the water block, on the water pipeline, into the same
// multisampled pair, and it reads the same set-3 background textures. That is
// what buys it Fresnel against the same sky the sea uses, the same refracted
// background, MSAA on its silhouette, and a depth test against the pool it
// pours into — with no sort, no blend and no second pass.
//
// **Nothing here is a look constant standing in for hydraulics.** How white the
// sheet is, is `aeration` out of the generated `loom_nappe_at`; how far it is
// thrown, is that function's throw; where it stops being a sheet, is its break
// length. What is left below is what those genuinely do not answer: how a
// stringy nappe is textured, and one authored term at the lip.
// ---------------------------------------------------------------------------

/// Strings across the sheet, in cycles over the whole lip.
///
/// A nappe leaving a rough stone lip is not smooth across its width — it leaves
/// in ligaments. This is how many of them, and 26 across a two-metre spout is
/// roughly the reference photograph's count.
static const float NAPPE_STREAK_ACROSS = 26.0;
/// Cycles per second of **departure time**, which is what makes the streaks
/// stretch as they fall — see `WaterOutput::nappe`. A metre of a fast sheet
/// covers less departure time than a metre of a slow one, so the same number
/// draws long smears low down and short ones at the lip, with no LOD term.
static const float NAPPE_STREAK_ALONG = 9.0;

/// How much of the sheet's opacity the entrained air is worth.
///
/// **Not 1.0**, because `aeration` is the air *fraction* of the sheet and not
/// its opacity: a half-aerated sheet is still mostly clear water with bubbles
/// in it, and painting it half-white makes a two-metre spout read as milk.
static const float NAPPE_OPACITY = 0.85;

/// The lip boil: an extra whitening in the first few centimetres,
/// `NAPPE_BOIL · exp(−Z / NAPPE_BOIL_DEPTH)`.
///
/// **Authored, and labelled as such.** In reference image 60 the whitest band
/// of the whole fall is at the lip, where `aeration` is by construction zero —
/// the water there has had no distance in which to entrain anything. What is
/// white there is *exit turbulence*: the channel behind the brink is already
/// broken up before it arrives. That is not in the free-fall model at all and
/// no coefficient in it stands in for it, so it is one exponential with two
/// numbers picked by eye rather than a derivation dressed up as one.
static const float NAPPE_BOIL = 0.7;
static const float NAPPE_BOIL_DEPTH = 0.3;

/// Where the sheet finishes turning into a curtain of drops, as a multiple of
/// the break length. `Z_b` is where the coherent core is used up; the sheet
/// does not vanish there, it opens.
static const float NAPPE_BROKEN_END = 1.8;

/// Metres of gap to the geometry behind over which the sheet fades in.
///
/// **At both ends, and it is the same number.** At the top the sheet leaves a
/// stone lip it intersects; at the bottom it enters the pool. Without this it
/// terminates in a hard straight line across both, which is the single most
/// obvious tell of a card standing in for a waterfall.
static const float NAPPE_DEPTH_FADE = 0.3;

/// **Where the sheet's silhouette comes from — and it must not be the strip's
/// own edge.** Third shipping of the quad-boundary class in this project, after
/// the smoke plume's ellipsoid and its cone; `SMOKE_RIM`/`SMOKE_EDGE` carry the
/// same reasoning and the same closed-form containment argument.
///
/// The old opacity chain was `aeration·NAPPE_OPACITY + boil` times
/// `lerp(0.55 + 0.9·strings, smoothstep(0.30, 0.72, strings), broken)`. Both
/// halves are *modulation*: the coherent branch is bounded below by 0.55 and
/// never reaches zero anywhere, so every fragment of `u ∈ [0, 1]` was drawn and
/// the visible boundary was exactly `lerp(lipA, lipB, u)` — a ruler-straight
/// line the noise cannot move, at any distance, from any camera.
///
/// The fix is the smoke's: raise the noise **threshold** across a band at the
/// rim, so out there water survives only where the ligament noise is high and
/// the boundary is an ISO-CONTOUR of that field — torn at the string frequency,
/// and moving down the fall because `strings` is read against departure time.
/// A blur or a distance-to-edge fade was explicitly not taken: a soft rectangle
/// is still a rectangle.
///
/// **The band's inner edge recedes with `aeration`, which is what keeps the lip
/// attached.** At `Z = 0` aeration is zero by construction — the water has had
/// no distance in which to entrain anything and a nappe leaving a chute is the
/// chute's width — so `start` is 1 and nothing is eroded. As the sheet aerates
/// the band opens to `NAPPE_RIM` and the edges fray. **The physics was already
/// there and simply not reaching the silhouette**: `aeration(Z)`, the break
/// length and the throw all drove *shading* and none of them drove *coverage*.
/// It is the same discrimination `broken` makes — `spout` reaches aeration 0.92
/// at its foot against `cascade`'s 0.80, and 0.60 against 0.42 near their lips,
/// so the thin spout frays harder and sooner than the canyon fall.
///
/// **The bound is closed form, which is why this is a band and not a fade.**
/// `strings` is `0.65·n₁ + 0.35·n₂` of `loom_value_noise`, which is a lerp of
/// `loom_lattice` values and so lies in `[0, 1)`; the amplitudes sum to 1, so
/// `strings < 1`. Coverage is `saturate((strings − thr) / NAPPE_W)` with
/// `thr = NAPPE_EDGE·rim − NAPPE_W`, so it is identically zero once
///
///     rim >= (1 + NAPPE_W) / NAPPE_EDGE
///
/// which at 2.2 / 0.35 is `rim >= 0.6136`, i.e. the support lies inside
///
///     |2u − 1| + rag < start + (1 − start) * 0.6136
///
/// At full fraying (`start = NAPPE_RIM = 0.56`) that is **0.8300**, and `rag`
/// is non-negative, so **no fragment is ever drawn outside |2u − 1| = 0.8300 —
/// a guaranteed 8.5% of the lip's width of clear air inside each geometric
/// edge**, 9.4 cm on `spout`'s 1.1 m lip and 34 cm on `cascade`'s 4 m one.
///
/// Measured over 4,000,000 samples of the actual two-octave `strings` on the
/// manifold the shader reads — `u` across the lip against eight seconds of
/// departure time — the max is **0.9376**, the 99.9th percentile **0.8360** and
/// the median **0.4970**, which put the drawn edge at |2u − 1| = 0.8175 worst
/// case, 0.7972 at the 99.9th and **0.7294 at the median**: 9.1%, 10.1% and
/// **13.5%** of the lip inside the geometry, before `rag` takes its bite. The
/// spread between those rows is the tearing, and it is why the guarantee is the
/// floor of the effect rather than a description of it.
///
/// What breaks the guarantee is lowering `NAPPE_EDGE` below `1 + NAPPE_W`, at
/// which point `thr` can no longer reach 1 and the straight edge comes back —
/// or letting `rag` push `side` *outward*, which is why it never does.
///
/// **A symmetric lateral wander was built, measured and deleted.** Shifting the
/// whole band's centreline off a low-frequency tap on departure time, so the
/// fall snaked, is indistinguishable from this at ±0.07 of half-width — because
/// `rag` is already sampled independently per side, so one edge biting while
/// the other does not *is* a centreline shift. Two constants, a noise tap and
/// 0.07 of the containment margin, for a picture nobody could tell apart.
static const float NAPPE_RIM = 0.56;
static const float NAPPE_EDGE = 2.2;
static const float NAPPE_W = 0.35;

/// How deep a bite the edge can take, in the same half-width units as
/// `NAPPE_RIM`, from what fraction of the tap's range, and at what rate down
/// the fall. See the use site: it only ever moves the edge *inward*, so the
/// containment bound above is unaffected by it.
static const float NAPPE_RAG = 0.34;
static const float NAPPE_RAG_BIAS = 0.40;
static const float NAPPE_RAG_HZ = 9.0;

/// How fast the fraying opens with `aeration`.
///
/// Aeration is zero at the lip by construction and reaches 0.92 at `spout`'s
/// foot and 0.80 at `cascade`'s, but it spends the first fifth of the fall
/// under 0.5 — and a fall whose top third is a clean-sided rectangle is still
/// a rectangle. Doubling it puts the sheet fully frayed by aeration 0.5, which
/// is 20 cm below `spout`'s lip and 1.2 m below `cascade`'s, while still
/// pinning the lip itself: `aeration(0) = 0` however this is scaled, so the
/// water still leaves the chute at the chute's width.
static const float NAPPE_FRAY = 2.0;

/// Where the sheet starts coming apart into the plunge pool, as a fraction of
/// the drop, and how much rim the foot is worth at the water line.
///
/// **The straight bottom edge is not a free edge and the obvious fix is a
/// no-op.** The last row sits exactly on the still-water plane, so the line
/// across the foot is the *intersection* of a continuous sheet with a flat
/// pool — dead straight by geometry, and there is nothing to occlude: running
/// the strip a quarter metre under the surface moves 162 pixels of `spout` and
/// **zero** of `cascade`, and two metres moves no more, because the strip
/// traces a curve and lengthening `v`'s range only redistributes rows along it.
/// A soft fade at the foot would be the rejected soft-rectangle again.
///
/// The only thing that removes a straight intersection is for the sheet not to
/// be continuous when it gets there, which is also what a real fall does — the
/// last stretch above a plunge pool is ligaments and drops, not a pane. So the
/// foot raises the same threshold, to `NAPPE_TOE_RIM` at the water line: with
/// `thr = 2.2 · 0.42 − 0.35 = 0.574` against a field whose median is 0.50, a
/// little over half the foot is gone and what lands is a row of separate
/// ligaments. **Not to the zero-guaranteeing 0.6136** — the sheet has to reach
/// the water, and a foot erased outright is a fall hanging in mid-air, which is
/// the worse artifact of the two.
static const float NAPPE_TOE = 0.72;
static const float NAPPE_TOE_RIM = 0.42;

float4 nappeColor(WaterOutput in) {
    float3 toEye = push.environment[0].eye.xyz - in.worldPos;
    float3 view = normalize(toEye);
    float3 normal = normalize(in.normal);
    if (dot(normal, view) < 0.0) {
        normal = -normal;
    }

    float aeration = in.nappe.y;
    float z = in.nappe.z;
    float departure = in.nappe.w;
    float u = in.baseXZ.x;

    // **Two octaves of the shared value noise, in (across, departure time).**
    // `loom_value_noise` is `loom_field::noise`, written in Rust and Slang side
    // by side and compared exactly — never a hand-rolled hash here (ADR 0006).
    float3 p = float3(u * NAPPE_STREAK_ACROSS, departure * NAPPE_STREAK_ALONG, 0.5);
    float strings = loom_value_noise(p) * 0.65
                  + loom_value_noise(p * 2.7 + float3(11.3, 5.1, 0.0)) * 0.35;

    LoomNappe nappe = loom_nappe_brink(push.environment[0].cascadeA.w,
                                       push.environment[0].cascadeB.w,
                                       push.environment[0].cascadeC.z);
    // **Past the break length the sheet opens into strings**, which is the
    // whole discrimination between the two reference waterfalls: a spout at
    // `q = 0.1` breaks at 1.31 m and is stringy for the last third of a
    // two-metre drop; a canyon fall at `q = 2` breaks at 9.7 m and never gets
    // there. Implemented as coverage on the same noise rather than as a second
    // particle system — the holes ARE the droplet curtain, and they are already
    // moving with the water.
    float broken = smoothstep(1.0, NAPPE_BROKEN_END, z / loom_nappe_break_length(nappe));

    float boil = NAPPE_BOIL * exp(-z / NAPPE_BOIL_DEPTH);
    float opacity = saturate(aeration * NAPPE_OPACITY + boil);
    // The strings modulate what is there; they do not invent whiteness in clear
    // water, which is why this multiplies rather than adds.
    opacity *= lerp(0.55 + 0.9 * strings, smoothstep(0.30, 0.72, strings), broken);

    // **The silhouette** — see `NAPPE_EDGE`. Deliberately the SAME `strings`
    // the sheet is textured with, so the fringe is the ligaments themselves
    // running out rather than a second pattern cut over the top of them: a
    // thick ligament survives further out than a thin one, which is what a
    // fraying sheet does. A second noise here reads as an eroded stencil.
    float fray = saturate(aeration * NAPPE_FRAY);
    // **Each edge eats inward on its own, and only inward.** `strings` is 26
    // cycles across the lip against 9 per second of departure time, which over
    // half a second of fall is barely five — so the ligaments are near-vertical
    // stripes, and an iso-contour of vertical stripes is a torn but still
    // plumb-straight line. A second low-frequency tap, sampled independently on
    // each side (the `sgn` shift), moves the boundary itself by up to
    // `NAPPE_RAG` — three string pitches — so the sheet narrows and widens in
    // patches as it falls. **Added to `side`, never subtracted**, so it can
    // only bring the edge in: that is what leaves `NAPPE_EDGE`'s containment
    // bound exactly as computed, the same rule the grass slope-wander follows.
    float sgn = u < 0.5 ? 0.0 : 5.0;
    // **Biased, so it bites rather than tapers.** Taking the tap raw makes the
    // mean bite half the amplitude everywhere, which narrows the whole fall
    // uniformly — a fall 40% thinner than its lip, evenly, which is a taper and
    // not a ragged edge. Through a smoothstep the edge sits near the band most
    // of the way down and takes a deep bite where the tap runs high.
    float rag = smoothstep(NAPPE_RAG_BIAS, 1.0,
                           loom_value_noise(float3(departure * NAPPE_RAG_HZ, sgn, 9.1)))
              * NAPPE_RAG * fray;
    float side = abs(2.0 * u - 1.0) + rag;
    float start = lerp(1.0, NAPPE_RIM, fray);
    float toe = saturate((z / max(push.environment[0].cascadeC.w, 1.0e-3) - NAPPE_TOE)
                         / (1.0 - NAPPE_TOE)) * NAPPE_TOE_RIM;
    float rim = max(saturate((side - start) / max(1.0 - start, 1.0e-3)), toe);
    opacity *= saturate((strings - (NAPPE_EDGE * rim - NAPPE_W)) / NAPPE_W);

    float2 screenUv = in.clip.xy / push.environment[0].viewport.xy;
    // The refracted background, offset by the sheet's own thickness. Water a
    // few centimetres thick bends the view ray by a few centimetres, so there
    // is no strength knob here either — the displacement is the physical one
    // and it shrinks with distance for free.
    float3 refracted = refract(-view, normal, 1.0 / WATER_IOR);
    float3 behindPos = in.worldPos + refracted * in.nappe.y;
    float4 offsetClip = mul(push.objects[push.objectOffset].mvp, float4(behindPos, 1.0));
    if (offsetClip.w > 0.0) {
        screenUv = saturate(offsetClip.xy / offsetClip.w * float2(0.5, 0.5) + 0.5);
    }
    float sceneDepth = opaqueDepth.SampleLevel(screenUv, 0).r;
    float3 behind = opaqueColor.SampleLevel(screenUv, 0).rgb;

    // **The fade at both ends.** How far the geometry behind this pixel is,
    // measured the way `waterBehind` measures it: unproject and compare true
    // distances, rather than subtracting two nonlinear depth values.
    if (sceneDepth < 1.0) {
        float3 bg = waterUnproject(screenUv, sceneDepth);
        float gap = length(bg - push.environment[0].eye.xyz) - length(toEye);
        opacity *= smoothstep(0.0, NAPPE_DEPTH_FADE, gap);
    }

    // **Discarded rather than drawn clear.** The sheet is opaque geometry in an
    // unblended pass, so a fully transparent fragment still writes depth — and
    // it would then occlude the mist behind the fall, which is exactly where
    // the mist is. Clear water at the lip is meant to be a hole.
    if (opacity < 0.01) {
        discard;
    }

    float3 whitewater = WATER_FOAM_ALBEDO
        * (hemisphereAmbient(normal) * ambientStrength()
           + sunColor() * sunStrength() * saturate(dot(normal, sunDirection())));
    float3 lit = lerp(behind, whitewater, opacity);

    // Fresnel against the same sky the sea reflects, so a fall and the pool
    // under it agree about what water does to light. Weakened by the aerated
    // part: a specular sheen belongs to a smooth surface, and past break-up
    // there is no surface left to have one.
    float fresnel = 0.02 + 0.98 * pow(saturate(1.0 - dot(normal, view)), 5.0);
    lit = lerp(lit, skyColor(reflect(-view, normal)), fresnel * (1.0 - 0.7 * aeration));

    float3 fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    return float4(lerp(lit, fogColor(-view), fog), 1.0);
}

[shader("fragment")]
float4 waterFragmentMain(WaterOutput in) : SV_Target {
    // **The fragment's world footprint, and it is taken HERE for a reason.**
    //
    // The water path's only derivative, and every statement below this one is a
    // place it could not have been taken. Slang lowers `discard` to `OpKill`,
    // not to a demote, so a derivative read after the shoreline discard reads a
    // dead lane in every quad that straddles the waterline — undefined, at
    // exactly the pixels `shore` and `river` exist to protect. The nappe
    // `return` above is the same hazard with a `return` in place of the
    // `discard`, and `in.nappe` is an interpolated varying, so no branch on it
    // is quad-uniform either. First statement of the entry point is the only
    // place all four lanes are certainly live — the same reason `quadShore`
    // asks its question before anything is killed, and the same trap
    // `flameColor` names where it takes its `fwidth` above its own early exit.
    //
    // Metres of world per pixel, on the horizontal plane, because the noise
    // this feeds is a function of `worldPos.xz` and nothing else. The larger of
    // the two screen axes: the footprint is an ellipse, and it is the major
    // axis that decides whether a lattice cell is resolved. This is what a
    // distance in metres cannot say — at a downward camera the same 40 m of
    // range is a centimetre per pixel underfoot and half a metre per pixel
    // toward the horizon, and the horizon is where the stipple was.
    float foot = max(length(ddx(in.worldPos.xz)), length(ddy(in.worldPos.xz)));
    // **And the foam frame's own footprint, here for exactly the same reason.**
    // The lace is not a function of `worldPos.xz`; it is a function of that
    // position rotated into the breaking frame, and only the derivative of the
    // rotated coordinate can band-limit it. See `foamOctaveWeight`.
    float3 foamFrame = foamFrameFoot(in.breakDir, in.worldPos.xz);
    // The falling sheet, which shares this pipeline and nothing else — ADR
    // 0054. Before the shoreline discard, because a nappe hangs in the air over
    // dry ground as often as not.
    if (in.nappe.x > 0.5) {
        return nappeColor(in);
    }
    // **The shoreline.** Where the bed is above the still-water line there is
    // no water, and drawing it anyway is a sheet of sea lying over a hill —
    // the single most obvious thing wrong with water that ignores terrain.
    //
    // A discard rather than a fade because the surface is opaque: it writes
    // depth, and a fade would need blending, a sort against the terrain behind
    // it, and a decision about what a half-transparent sea reflects. The edge
    // is not left raw, though — the shoaling taper flattens the waves to
    // nothing as they reach it, so the cut runs along still water rather than
    // through a moving crest, which is what would make it crawl.
    //
    // **Asked as a quad-wide question first, because the reflection share far
    // below reads the other three lanes.** Slang lowers `discard` to `OpKill`,
    // not to a demote, and a quad read from a killed lane is undefined — so a
    // shoreline quad, where some lanes are water and some are hill, is exactly
    // the hazard `fragmentMain` avoids by keeping alpha-cutout materials off
    // the sharing path. Here the condition is an *interpolated varying* rather
    // than a material constant, so no branch can be quad-uniform about it: it
    // has to be measured. Three quad reads, in uniform control flow, before
    // anything is killed; `quadShore` is then a quad-uniform value the share
    // can branch on. A partly-covered quad reads its helper lanes' extrapolated
    // depth and may answer "shore" when it is not one — which costs that quad
    // the share and nothing else, and only on a silhouette.
    float shore = (in.depth <= 0.0) ? 1.0 : 0.0;
    float quadShore = shore + QuadReadAcrossX(shore) + QuadReadAcrossY(shore)
                            + QuadReadAcrossDiagonal(shore);
    if (shore > 0.0) {
        discard;
    }

    float3 toEye = push.environment[0].eye.xyz - in.worldPos;
    float3 view = normalize(toEye);
    float3 normal = normalize(in.normal);
    // Seen from underneath — the far wall of a trough, and eventually W7's
    // underwater camera — the analytic normal points away. Flipping it keeps
    // the shading continuous instead of letting the surface go black.
    if (dot(normal, view) < 0.0) {
        normal = -normal;
    }

    // **Seen from below, which is a different surface.** Everything above this
    // line is shared; everything after it assumes the sky is on the far side,
    // and applying it from underneath draws the underside of a lake as a
    // bright sky-lit sheet — which is what being submerged looked like until
    // this branch existed. This is a bug fix, not an effect.
    //
    // The flag is the camera's, not this pixel's: the normal flip above
    // already handles the far wall of a trough seen from *above*, and that
    // pixel must keep being shaded as the top of the water it is.
    if (eyeUnderwater()) {
        // **Schlick against the angle on the air side.** Going water → air the
        // reflectance climbs to 1 at the critical angle and stays there — total
        // internal reflection — so the ordinary form, which is 0.02 face-on and
        // 1.0 only at exactly 90°, is not merely inaccurate here but misses the
        // effect entirely. Feeding the *transmitted* angle to the same Schlick
        // polynomial reaches 1.0 exactly where `sinT2` does, so the rim is
        // continuous rather than a drawn ring — though only just: measured, it
        // goes from a tenth reflected to all of it over the last two degrees,
        // which is why the window has a hard edge in the reference and in every
        // photograph of one.
        float cosI = saturate(dot(normal, view));
        float sinT2 = WATER_IOR * WATER_IOR * (1.0 - cosI * cosI);
        float3 through = float3(0.0, 0.0, 0.0);
        float fresnel = 1.0;
        if (sinT2 < 1.0) {
            float cosT = sqrt(1.0 - sinT2);
            fresnel = 0.02 + 0.98 * pow(saturate(1.0 - cosT), 5.0);
            // **Snell's window**, and it costs one `refract`: the entire sky
            // above, squeezed into a 97°-wide disc overhead, everything outside
            // it mirror. `skyColor` is the same function the background pass
            // draws, so the sun rides along in the window where a real one is.
            through = skyColor(refract(-view, normal, WATER_IOR));
            // **The web on the underside of the ceiling.** Seen from below,
            // the surface is the lens rather than the bed being lit by it —
            // the same convex patches that converge sunlight into a filament
            // on the sand are bright when you look straight at them, so the
            // ceiling carries the identical pattern. It is the most
            // recognisable thing about looking up from underwater and it is
            // the same `causticWeb` at the surface's own position, with no
            // depth to fade it.
            //
            // Applied to the transmitted light only, not to the mirror: the
            // total-internal-reflection region outside Snell's window is
            // showing the seabed, not the sky, and has no business carrying a
            // pattern made by light coming down.
            through *= 1.0 + causticWeb(in.worldPos, 0.0) * UNDERSIDE_CAUSTIC;
        }
        // What the underside reflects is the water itself — and since the
        // medium is one colour in every direction, the mirror comes out
        // *identical to the background*. **That is a real limitation, not an
        // oversight**: a real mirrored surface carries a reflection of the
        // seabed, and this one is featureless everywhere outside the window.
        // Fixing it needs the geometry behind the surface, which is the
        // refraction/reflection pass this step is explicitly not building.
        // **The mirror looks where a mirror looks.** This passed `-view`, which
        // is the direction back to the eye, not the direction the surface
        // reflects toward — harmless while the medium returned one colour for
        // every direction, and wrong the moment it stopped. Outside Snell's
        // window the underside mirrors the seabed, so it should be reading the
        // dark downward part of the column, and now it does.
        float3 lit = lerp(through, underwaterColor(reflect(-view, normal)), fresnel);
        float3 fogUnder = fogAmount(push.environment[0].eye.xyz, in.worldPos);
        return float4(lerp(lit, fogColor(-view), fogUnder), 1.0);
    }

    // **Capillary ripples, applied to the normal only.** This is what stops a
    // swell face lighting as one flat facet: the specular lobe gets something
    // finer than the mesh to break it across, which is the difference between
    // a sheet and a surface.
    //
    // Faded to nothing by `WATER_DETAIL_RANGE`, because the lattice goes under
    // a pixel out there and analytic noise has no mip chain to catch it.
    // Calmed in the shallows by the same depth the shoaling uses, since water
    // over a beach really is smoother than water over the shelf.
    // The mesh's own normal, kept before the capillary ripples tilt it. The
    // reflection ray is offset along THIS and clamped to its hemisphere: a
    // ripple can tilt the shading normal several degrees, and a reflected ray
    // that ends up pointing below the real surface starts inside the seabed
    // and comes back with a lit sand sample — one bright pixel among dark
    // ones, which is what a firefly is.
    // **The wake, re-read per pixel — W10.** The vertex shader samples the grid
    // at the mesh's own cells, which in the near ring are 0.5 m: the same
    // spacing as the grid itself, so a two-metre ring arrives as five linear
    // segments and its slope as a staircase. The base point is a linear
    // varying, so sampling here is the exact value at this pixel.
    //
    // `dWavelet` is the *difference* from what the vertex already folded into
    // `normal`. Adding the whole slope again would draw the wake twice.
    //
    // **Branched on `count > 0`, and that is not an optimisation.** Water
    // nothing has touched must render bit for bit as it did, and
    // `normalize(normal - 0)` is not a no-op — it is a second normalise of an
    // already-normalised vector, which moves the last bit. Measured: `ocean`
    // and `water_crate` both came back byte-different from their references
    // with this unguarded, inside the image gate's tolerance and therefore
    // exactly the kind of drift only `cargo xtask repeat` can see.
    //
    // **This is a per-pixel sum over the pool** rather than a texture lookup,
    // and it is the same loop the vertex shader runs: a ring's slope across one
    // mesh cell is not linear, and interpolating it draws the crest as a facet.
    LoomWaveletPool pool = waveletPool();
    if (pool.count > 0) {
        LoomWavelet wavelet = loom_wavelets_at(
            pool, in.baseXZ, push.environment[0].weather.z);
        float2 dWavelet = wavelet.slope - in.waveletSlope;
        normal = normalize(normal - float3(dWavelet.x, 0.0, dWavelet.y));
    }

    // **The rain's own rings, and the gate is three per-position terms.** The
    // rate is the scene's unsheltered one; `cloudCoverAt` is where the shower
    // actually is right now (not `cloudCoverRecent`, which is wetness — an
    // integral — and would leave rings on water the shower left a minute ago);
    // and the ground test is the only shelter this shader can see. A scene
    // with no rain multiplies to zero and pays one compare.
    float ringGate = saturate(push.environment[0].rain.w / RAIN_FULL_RATE);
    if (ringGate > 0.0) {
        float bed = loom_ground_height(terrainField(), in.worldPos.xz);
        float open = in.worldPos.y > bed - WET_COVER_SLACK ? 1.0 : 0.0;
        ringGate *= open * cloudCoverAt(in.worldPos);
    }
    float ringEnvelope = 0.0;
    float2 ringSlope = rainRingsAt(
        in.worldPos.xz, push.environment[0].weather.z, ringGate, ringEnvelope);

    float3 flatN = normal;
    {
        float detailFade = saturate(1.0 - length(toEye) / WATER_DETAIL_RANGE);
        // Calmer over a beach than over the shelf, which is true of real water
        // and keeps the ripples off the very edge of the shoreline cut.
        detailFade *= saturate(in.depth * 0.7 + 0.2);
        // **And calmer inside a ring's envelope, which is what makes the ring
        // a shape rather than a texture.** The contest is not close on its own
        // terms: a ring tilts the normal by 0.06–0.12 against this
        // unconditional 0.30 of capillary noise with a wind floor under it.
        // Water really does go glassy where a wave passes under it, because the
        // orbital motion stretches the surface — so the term that hides the
        // ring is exactly the term the ring should suppress. 0.7 rather than
        // 1.0 because a mirror-flat band reads as a hole.
        //
        // This is `RIPPLE_CALM`'s mechanism, relocated to where it is earned
        // (ADR 0053 §7): it used to hang off the CPU wake grid's slope and
        // applied wherever that grid reached.
        detailFade *= 1.0 - 0.7 * ringEnvelope;
        // **The wind, read where this pixel is — W11.** `grassWind` is the
        // same generated `wind_at` the grass bends to and the particles drift
        // in, so a gust that runs across a meadow runs across the pond beside
        // it, out of one field rather than two that can disagree (ADR 0006).
        // Per pixel and not from `wind.z`, because a global scalar can only
        // roughen the whole sea in unison: a cat's paw is a *patch* of
        // roughened water travelling downwind, and that is the thing being
        // asked for.
        float4 windv = push.environment[0].wind;
        float2 dw = normalize(windv.xy + float2(1e-4, 0.0));
        float local = length(grassWind(in.worldPos).xz);
        float detailStrength = sqrt(WATER_DETAIL_CALM + WATER_DETAIL_COXMUNK * local);
        // **And carried by the river — W8.** The current is the one advection
        // in this shader that is honestly per-pixel: it is a bake off the bed,
        // it is what the buoyancy solver integrates, and a river whose surface
        // texture stands still while a crate floats past it is the single most
        // visible thing wrong with `river.loom`.
        float2 g = waterDetailSlope(
            in.worldPos.xz,
            push.environment[0].weather.z,
            dw * windv.z * WATER_DETAIL_DRIFT,
            loom_flow_at(flowField(), in.worldPos.xz));
        // A slope subtracted from the normal's horizontal components — the
        // standard perturbation. The surface stays exactly where the vertex
        // shader put it; only how it catches light changes, which is what keeps
        // this out of the "second opinion about the drawn surface" that the
        // rejected wave-set version would have been.
        normal = normalize(normal - float3(g.x, 0.0, g.y) * detailStrength * detailFade);
        // **The rings, added after the detail they suppress.** Subtracted like
        // every other slope in this block, so the surface stays exactly where
        // the vertex shader put it and only the shading normal moves. Branched
        // for the reason every other water branch here is branched: a scene
        // with no rain must render bit for bit as it did, and
        // `normalize(normal - 0)` is a second normalise of an already-unit
        // vector, which moves the last bit.
        if (ringGate > 0.0) {
            normal = normalize(normal - float3(ringSlope.x, 0.0, ringSlope.y));
        }
    }

    // Schlick against water's f0. This is the whole trick: face-on you see
    // into the water, at a grazing angle you see the sky, and the crossover is
    // most of what makes a surface read as liquid.
    // **Fresnel that knows the surface is rough, which is what lets you see
    // into the water.**
    //
    // Schlick's polynomial goes to 1.0 at grazing on a PERFECTLY SMOOTH
    // surface. A real sea is covered in ripples, so at a grazing angle a good
    // fraction of the microfacets are not grazing at all and the reflectance
    // never reaches one — the standard roughness-aware correction, and the
    // reason a real beach lets you see the bottom at angles where a mirror
    // would show only sky.
    //
    // Without it the horizon-ward half of every water frame washes to sky and
    // the seabed the refraction went to such trouble to fetch is invisible
    // exactly where there is most water to look through.
    float grazing = pow(saturate(1.0 - dot(normal, view)), 5.0);
    float fresnelCap = max(1.0 - WATER_CLARITY_ROUGH, 0.02);
    float fresnel = 0.02 + (fresnelCap - 0.02) * grazing;

    float toSun = max(dot(normal, sunDirection()), 0.0);

    // **Sky occlusion by wave shape, and it buys more than it costs.** A trough
    // sees a slice of the hemisphere; a crest sees all of it. That difference
    // is most of why the troughs in a photograph of a rough sea go nearly
    // black while the crests stay bright, and without it a sea lit by ambient
    // is uniformly lit no matter what shape it is in. One multiply.
    float crest = saturate(in.fold / WATER_FOLD_REF);
    float skyView = saturate(0.55 + 0.45 * crest);

    float2 screenUv = in.clip.xy / push.environment[0].viewport.xy;
    float3 behind;
    // **The ray bends here, once, at the real index.** `refract` takes the
    // incident direction — from the eye INTO the surface — and `eta` as
    // n_incident/n_transmitted, which is the convention the underwater branch
    // above already pins by passing `WATER_IOR` for the water->air trip. Going
    // the other way it is the reciprocal.
    //
    // **No total-internal-reflection guard, deliberately.** Air->water gives
    // `sinT2 <= 1/1.333^2 = 0.5628` at every incidence angle, so `refract`
    // cannot return zero here, and a guard would advertise a case that does
    // not exist.
    float3 refracted = refract(-view, normal, 1.0 / WATER_IOR);
    float column;
    float gap;
    float3 T = waterBehind(screenUv, in.worldPos, push.environment[0].eye.xyz,
                           in.depth, refracted, behind, column, gap);

    // **Backlit scatter, gated on how THIN the water is — not on view angle,
    // and, as of this commit, not on steepness either.**
    //
    // The term this replaces was `body * 6.0 * pow(dot(-view, sunDir), 3)`,
    // which glows a trough exactly as brightly as a crest and so reads as haze
    // laid over the whole sea rather than as light coming through a wave. A
    // real crest glows because it is a thin sheet with the sun behind it, and
    // thickness is what decides how much survives the crossing.
    //
    // **`crest` was the wrong gate over a beach and that is what cost `shore`
    // its turquoise.** Shoaling flattens the swell as it comes onto the shelf,
    // so `in.fold` — and with it `crest` — goes to zero across exactly the
    // shallow band where a real sea glows most. The term was gated on the one
    // quantity guaranteed to vanish where it was needed. It gained the column
    // `waterBehind` measures: light that crossed two metres of water and came
    // back up is thin-water scatter whatever shape the surface is in.
    //
    // **`crest` was the wrong gate over the open sea too, and that is what this
    // commit fixes.** Off a bed the column is the sentinel, the shallow half of
    // the gate is identically zero, and the mask was `crest` alone — an
    // absolute threshold on *steepness*, on a quantity whose scale is the sea's
    // own. A Beaufort 4 sea never reaches it, so `ocean_tropical` — a 0.81 m
    // Pierson–Moskowitz sea framed from 1.1 m above its own waterline, which is
    // the photograph this whole optics chain exists to make — got no subsurface
    // light at all. But a gentle groundswell's crest is exactly as much a thin
    // sheet with the sun behind it as a storm wave's is; being a sheet is a
    // fact about **elevation**, not about steepness, and every photograph of a
    // backlit swell says so. So the mask is now `lift`: how high this point
    // stands above the still line, in units of this sea's own significant crest
    // (`waterCrestScale`, `2σ`). It saturates on the same 2.3% of a Gaussian
    // surface at every sea state, where `crest` saturated 3.1% of `ocean` and
    // ~0% of a Beaufort 4 sea.
    //
    // **A glassy sea has `σ = 0` and no crest**, so `lift` is zero over the
    // whole surface and the term is off — which is the property `crest` used to
    // supply through the fold bound and which had to survive the change.
    //
    // **Steepness has not gone; it has stopped being a switch.** `crest` still
    // sets `skyView` above, which is a question about surface shape.
    //
    // **The hue is not authored**
    // — it falls out of `waterExtinct()` through Beer-Lambert, so the colour of
    // a backlit wave is the colour water actually transmits: through 2 m that
    // is (0.51, 0.89, 0.97), the luminous turquoise a breaking crest has.
    float crestScale = waterCrestScale();
    float lift = crestScale > 0.0
        ? saturate((in.worldPos.y - stillWaterLevel()) / crestScale)
        : 0.0;
    // **The crossing is whichever of the two transmits more**, which is what
    // `min` of two path lengths says under Beer-Lambert: `max(exp(-Ka),
    // exp(-Kb)) == exp(-K·min(a, b))`. Light reaches the eye either by crossing
    // the crest or by going down to a bed and back, and the shorter path is the
    // one that carries the colour.
    //
    // **It was `column` alone wherever there was a bed, and that is the second
    // half of `ocean_tropical`'s defect.** Its sand shelf is five metres down,
    // so the measured column is 12–14 m over the whole frame — past
    // `WATER_GLOW_COLUMN` (so the shallow gate never opened either) and thick
    // enough that what little survived arrived navy. A five-metre bed does not
    // make a crest above it a thicker sheet.
    //
    // `WATER_TROUGH_THICK` is still the only answer available for the crest
    // crossing itself and is still the authored 4 m, measured against nothing.
    float sheet = WATER_TROUGH_THICK / max(sunDirection().y, 0.15);
    float thick = min(column, sheet) * lerp(1.0, WATER_CREST_THIN, lift);
    // **The lobe is gone, and the geometry is why it could never have worked.**
    //
    // What stood here was `pow(saturate(dot(-view, normalize(sunDirection() +
    // normal * 0.35))), 4.0)` — a wrap lobe with two authored constants and no
    // referent for either. It evaluated to 0.02-0.11 on `ocean_tropical`, so
    // the term arrived as a lift rather than a luminance, and its peak is
    // provably below the waterline: maximising over `view` wants
    // `view = -(L + N*d)`, whose normal component is `-(L.N) - d < 0`.
    //
    // **The honest replacement was tried first and falsified.** Both in-water
    // directions exist here — `refracted` for the eye, `refract(-sunDirection(),
    // normal, 1/WATER_IOR)` for the sun — so the single-scattering angle can be
    // taken for real. It cannot help, and that is a theorem rather than a
    // measurement: refraction confines both rays to the Snell cone, half-angle
    // `asin(1/1.333) = 48.6 deg`. The sun's in-water ray goes DOWN inside it and
    // the ray that leaves for an above-water eye goes UP inside it, so with
    // `a, b <= 48.6 deg` off the vertical, `cos(theta) <= -cos(a + b) <=
    // -cos(97.2 deg) = 0.125`: **the scattering angle is at least 82.8 degrees
    // for every camera, every sun and every wave face this engine can author.**
    // Over 82.8-180 deg the Henyey-Greenstein phase function at `g = 0.9186` —
    // the asymmetry whose backscatter fraction is exactly the Petzold 0.0183
    // that `WaterOptics` builds its `backscatter` column from — runs from
    // 0.00606 sr^-1 down to 0.00176, which is **0.076 to 0.022 of isotropic**;
    // and over the angles a real frame actually produces (92-121 deg measured
    // across `ocean_tropical`) it is 0.058 to 0.033, **a constant to within a
    // factor of 1.8**. There is no single-scattering lobe to have: the sun's
    // forward peak is unreachable from above the water, and what it would buy
    // if it were reachable is a factor of two. A backlit crest is not showing
    // you a forward peak; it is showing you multiply-scattered light.
    //
    // **And this renderer already carries the coefficient for that.**
    // `waterExtinct()` is `K_d`, the DIFFUSE attenuation coefficient, and
    // `WaterOptics::attenuation`'s own documentation says why it is not the
    // beam coefficient: forward-scattered light is not lost, it goes on to
    // reach the eye. Beer-Lambert on `K_d` is the multiple-scattering transport
    // model. Multiplying it by a single-scattering lobe double-counts a
    // direction the diffusion has already smeared out.
    //
    // **So the only directional fact a thin sheet licenses is Lambert's cosine
    // on the face the light enters**, and the two paths enter through different
    // faces:
    //
    // - A crest is a sheet with the sun BEHIND it. In the parallel-faced sheet
    //   this comment has asserted since the term was written, the entry face's
    //   normal is `-normal`, so the entry cosine is `-dot(normal, L)`. It is
    //   the exact complement of `toSun` above: the sun either lights the face
    //   you can see or it lights the one behind it, never both.
    // - A shallow column is lit from ABOVE — down through the surface you are
    //   looking at, and back up. Its entry face is the one you see, so its
    //   entry cosine is `toSun` itself.
    //
    // Zero authored constants where there were two, and the horizon band that
    // killed the sign-flip experiment cannot form: far-field water faces up,
    // the sun is above it, `backlit` is zero there by construction.

    // **The single-scattering phase function, at the angle the geometry
    // actually offers**, and `g` is not authored: it is the Henyey-Greenstein
    // asymmetry whose backscatter fraction
    // `b_b/b = (1-g)/(2g) * ((1+g)/sqrt(1+g*g) - 1)` equals **0.0183**, the
    // Petzold average-particle fraction `WaterOptics` already builds its whole
    // `backscatter` column out of. Inverting that gives g = 0.9186, which is
    // the value Mobley quotes for the same reason.
    //
    // `WATER_PHASE_G`'s own doc carries the inversion, and `loom_render`'s
    // `the_phase_asymmetry_is_petzold_s_backscatter_fraction` re-runs it
    // against the literal so it cannot drift.
    float3 sunIn = refract(-sunDirection(), normal, 1.0 / WATER_IOR);
    float cosScatter = dot(sunIn, -refracted);
    float gg = WATER_PHASE_G * WATER_PHASE_G;
    float pd = max(1.0 + gg - 2.0 * WATER_PHASE_G * cosScatter, 1e-4);
    // Normalised to isotropic (`4*pi*p`), so 1.0 would be "as much as a
    // uniformly scattering medium sends this way". Bounded by the theorem
    // above to 0.022-0.076 for every above-water camera in this engine.
    float phase = (1.0 - gg) / (pd * sqrt(pd));
    float backlit = saturate(-dot(normal, sunDirection()));
    float shallow = saturate(1.0 - column / WATER_GLOW_COLUMN);
    float3 sss = sunColor() * sunStrength()
               * max(lift * backlit, shallow * toSun * phase)
               * exp(-waterExtinct() * thick);
    // Ablation, at the term's single point of use rather than at each of the
    // factors feeding it, exactly as `LOOM_ABLATE_WATER_FOAM` is applied: what
    // is measured is then "the light this frame brought up through the water"
    // and not one contributor to it.
    if (ablated(LOOM_ABLATE_WATER_GLOW)) { sss = float3(0.0, 0.0, 0.0); }

    // **What the eye actually receives from below, split by how much water is
    // in the way.** `T` is the fraction that crossed the column; `1 - T` is
    // the fraction that scattered before it got there.
    //
    // The old form was `body * downwelling`, where `body` interpolated
    // `WATER_SHALLOW` toward the deep colour by an authored bed depth. That was a
    // hand-picked *linear* stand-in for exactly this, with no view-angle
    // dependence, and it is deleted rather than supplemented — this is the
    // decomposition it approximated. `L -> infinity` gives `waterDeep() *
    // downwelling`, which is what it used to answer everywhere; `L -> 0` gives
    // the background unattenuated, which is the shoreline.
    //
    // **The partition conserves energy**, which is why adding a seabed makes
    // the water deeper rather than brighter.
    float3 downwelling = hemisphereAmbient(normal) * ambientStrength() * skyView
                       + sunColor() * sunStrength() * toSun;
    // `skyView` scales the volume term ONLY. `behind` was lit by its own
    // shading pass; darkening it in a trough would dim the seabed for a reason
    // that has nothing to do with the seabed.
    float3 through = behind * T + waterDeep() * downwelling * (1.0 - T) + sss;

    // **The sea reflects the scene now, and it is one ray.** This was
    // `skyColor(reflect(...))` — an analytic sky and nothing else, so a quay
    // standing in the water reflected the weather and not the quay. ADR 0019
    // already put an inline ray query in this file with a sky fallback and a
    // radiance clamp; the only reason water did not use it was that its miss
    // colour was hard-coded to `skyGradient`, which would have deleted the
    // sun's glitter path. `miss` is that one parameter.
    //
    // **Clamped into the surface's own hemisphere first.** See `flatN`.
    //
    // **`WATER_REFLECT_ROUGH`, not `WATER_ROUGHNESS`.** The lobe widening
    // inside `tracedEnvironment` also blends the traced answer toward the miss
    // colour, reaching it at 1.0 — so this number is how much sky a reflected
    // shed is diluted with, and 0.08 would be reading the sun-glint lobe's
    // width as if it were that. The capillary ripples already do the
    // broadening geometrically, per pixel, which is the honest version of it.
    float3 mirror = reflect(-view, normal);
    mirror = normalize(mirror - flatN * min(dot(mirror, flatN), 0.0));
    if (in.nappe.x < -0.5) {
        // **One ray, jittered inside the lobe — the marched surface only.**
        // See `WATER_CINEMATIC_LOBE`. The sample count does not move; what
        // moves is that two neighbouring pixels no longer ask the same
        // question, which is what a flat straight-edged patch *is*.
        //
        // This used to cite ADR 0019's occupancy cliff as the reason more rays
        // were refused. **That cliff does not reproduce** — see the ladder in
        // `aoRays` — so the refusal now rests on what actually measured it:
        // the fifty-fold salt result below. A rejection resting on a fact that
        // no longer holds is a trap for whoever reads it next.
        //
        // `+ float2(19.0, 7.0)` for the reason `ambientVisibility` offsets by
        // `(37, 11)`: two consumers of `rayDither` sharing one pattern
        // correlate, and ADR 0019 paid for that lesson once.
        float3 u, v;
        rayBasis(mirror, u, v);
        float2 xi = raySample(rayDither(in.clip.xy + float2(19.0, 7.0)), 0u);
        float radius = WATER_CINEMATIC_LOBE * sqrt(xi.x);
        float angle = 6.2831853 * xi.y;
        mirror = normalize(mirror + u * (radius * cos(angle))
                                  + v * (radius * sin(angle)));
        // Back into the surface's own hemisphere, exactly as above: a sample
        // rotated below the water cannot reflect anything.
        mirror = normalize(mirror - flatN * min(dot(mirror, flatN), 0.0));
    }
    float baseRough = (in.nappe.x < -0.5) ? WATER_CINEMATIC_REFLECT_ROUGH
                                          : WATER_REFLECT_ROUGH;
    float reflectRough = saturate(baseRough
                                + length(toEye) / WATER_REFLECT_RANGE);
    float3 env = tracedEnvironment(in.worldPos, flatN, mirror,
                                   reflectRough, WATER_REFLECT_BIAS,
                                   skyColor(mirror));
    if (in.nappe.x < -0.5 && quadShore == 0.0) {
        // **Four rays' worth of answer at one ray's price.** The quad is
        // already resident and already shading; sharing across it turns four
        // independent jittered samples into one averaged reflection, which is
        // what stops the jitter from trading a coherent polygon for per-pixel
        // stipple. Unshared it is measured at fifty times the added salt —
        // that number is what chose this design, not the cost.
        //
        // **Quad-uniform, and it takes both halves to say so.** `nappe.x` is a
        // per-vertex constant of -1.0 across the whole fluid mesh, so every
        // lane of a quad on this surface agrees about the first test; helper
        // lanes included. `quadShore` is the second half and is the one that
        // was missing: the shoreline `discard` far above is gated on an
        // interpolated depth, so a quad straddling the waterline had lanes that
        // had already executed `OpKill` when this read them. That is undefined,
        // not merely wrong, and it is the same hazard `ambientVisibility`
        // documents. It reads lanes of a 2x2 fixed by the pixel grid rather
        // than by scheduling, so it carries no more nondeterminism than the
        // dither does.
        env = (env + QuadReadAcrossX(env) + QuadReadAcrossY(env)
                   + QuadReadAcrossDiagonal(env)) * 0.25;
    }
    float3 lit = lerp(through, env, fresnel);
    lit += specular(normal, view, sunDirection(), WATER_ROUGHNESS, float3(0.02))
         * sunColor() * sunStrength() * toSun;

    // **Whitecaps.** See the constants above for why the threshold is absolute.
    //
    // The wind comes straight from the environment buffer rather than from
    // `grassWind`. `grassWind` is `wind_at`, which the generated tree spends
    // three noise taps and nine `sin` on — 2.5x this entire block — to add
    // gusting and turbulence to a *direction* that here only needs to say which
    // way the streaks lie.
    float4 windv = push.environment[0].wind;
    // **The frame the streaks lie in is the local breaking direction now**, not
    // the wind — ADR 0053 §7. The wind still carries the patch (below); what it
    // no longer does is decide which way the lace is combed, which is a
    // property of the crest that entrained the air.
    //
    // Renormalised because it is interpolated: two vertices whose crests differ
    // in heading give a short vector in between, and a short vector here is a
    // stretched lattice rather than a rotated one. The `1e-4` keeps flat water
    // — where the eigenvector is genuinely undefined — off a zero-length
    // normalize; nothing there draws foam anyway. Both are done in
    // `foamFrameFoot`, at the top of this function, because the same frame has
    // to be differentiated there and a second `normalize` here could differ
    // from it in the last bit.
    float2 wd = foamFrame.xy;
    float2 windDir = normalize(windv.xy + float2(1e-4, 0.0));
    // Foam drifts downwind. Sampling in a frame that slides with the wind is
    // what keeps a patch from sitting nailed to the world while the wave that
    // made it travels out from under it.
    float2 wp = in.worldPos.xz - windDir * windv.z * push.environment[0].weather.z * 0.05;
    // **And downstream, where there is a stream — W8.** Foam floating on a
    // river goes where the river goes; the wind drift above is the open-sea
    // answer and is a few percent of a wind speed, which on `river.loom` is
    // nothing at all beside a 3 m/s current.
    float2 fflow = loom_flow_at(flowField(), in.worldPos.xz);
    float3 fw = waterFoamWeights(foamFrame.z);
    float fnoise;
    // The raft's own texture — see `waterFoamRaft`. It rides the same two flow
    // phases `fnoise` does, for the reason `waterDetailSlope` gives: a river's
    // foam has to go where the river goes, and still water has to come out bit
    // for bit as it did.
    float raft;
    if (dot(fflow, fflow) <= 0.0) {
        fnoise = waterFoamNoise(wp, wd, fw);
        raft = waterFoamRaft(wp, wd);
    } else {
        LoomFlowPhase fp = waterFlowPhase(wp, fflow, push.environment[0].weather.z);
        fnoise = waterFoamNoise(fp.uvA, wd, fw) * fp.wA
               + waterFoamNoise(fp.uvB, wd, fw) * (1.0 - fp.wA);
        raft = waterFoamRaft(fp.uvA, wd) * fp.wA
             + waterFoamRaft(fp.uvB, wd) * (1.0 - fp.wA);
    }
    // How wide the coverage is eroded through: 0.15 while the lace is whole,
    // opening out to the noise's own spread as the ladder retires it.
    float ferode = waterFoamErode(fw);
    // **The trail is the coverage's floor, not an addition to it.** A point
    // under a steepening crest is foamy because of the crest; a point the
    // crest has left is foamy because of what the crest did. Taking the
    // maximum keeps a fresh whitecap exactly as bright as it was before W2 and
    // lets the old foam persist behind it, which is what a `max` of a coverage
    // means. Adding them would make a crest passing over its own wake brighter
    // than paper.
    // **The wake's white crest is gone from here — ADR 0053 §7.** It was
    // `ripSteep * RIPPLE_FOAM_MAX`: the CPU ripple grid's own slope, read as a
    // coverage, painted every frame with no memory at all. A wake's foam is
    // the one foam in this shader that genuinely cannot be evaluated backwards
    // in time (ADR 0049's own "what would change this"), so painting it from
    // the instantaneous slope made it a highlight welded to the ring — it
    // appeared and vanished with the ring instead of being left behind by it.
    // The mechanism moves to the foam field's hull and impact deposits, which
    // remember. The ring's slope still shapes the normal above.
    // **The threshold wanders — see `WATER_FOAM_PATCH`.** Both ends by the same
    // non-negative amount, so the gate keeps its meaning and a gentle sea still
    // foams nowhere.
    //
    // **Gated on the unwandered floor, which is exact rather than an
    // approximation.** The wander only ever raises the pair, so a point whose
    // `mu_max` has not reached the *base* `WATER_FOAM_WET` cannot foam under
    // any wander this can return — the same guarantee the gentle-sea property
    // rests on, spent here as a skip. Two `loom_value_noise` taps are not
    // cheap: unconditional they cost **+0.181 ms** of the water pass on
    // `ocean_fft_boat` at 1920x1080 (0.256 -> 0.437, min of 80 frames), which
    // is as much as evaluating the whole cascade per fragment would. Past 0.22
    // is 9.9% of a wind sea's samples (`WaterSample::mu_max`'s own table), so
    // most quads skip both.
    float breakUp = 0.0;
    float instant = 0.0;
    if (in.breakMu > WATER_FOAM_WET) {
        breakUp = waterFoamBreakWander(in.baseXZ, push.environment[0].weather.z, fw.x);
        instant = smoothstep(WATER_FOAM_WET + breakUp, WATER_FOAM_BREAK + breakUp, in.breakMu);
    }
    // **The advected field, and it is a third floor rather than a second
    // opinion** — ADR 0055. The trail remembers what a *wave* did, because a
    // wave is the only thing whose past is a closed form in `(x, t)`; this
    // remembers what a hull, an impact and a wake ring did, none of which can
    // be evaluated backwards from a position. `max` for the reason the trail
    // is a max: foam is a coverage, and a crest crossing its own wake must not
    // come out brighter than paper.
    float field = loom_foam_at(foamField(), in.baseXZ);
    float wetc = max(max(instant, in.foamHist), field);
    // **How old this foam is, from the two numbers already in hand.** A point
    // whose whiteness is entirely history is old; one the crest is making right
    // now is fresh; in between it is the ratio. Free — no varying, no state.
    //
    // Koepke: whitecap reflectance falls 55% -> 3–10% over about ten seconds as
    // the raft drains. Fresh foam is bright and opaque, old foam is grey and
    // full of holes, and painting both with one albedo is what made a trail
    // read as a smear of paint rather than as foam that had been there a while.
    // **The epsilon is not tidiness: without it this is 0/0 on every pixel the
    // field alone made white**, and a NaN age lerps the whole shaded result to
    // NaN, which resolves to black. Measured — `whitecaps` came back with the
    // foam field's footprint punched out of it in solid black.
    //
    // **The field is deliberately not in the numerator.** Its foam is drawn
    // fresh: a hull's churn and an impact's ring are white while they are being
    // made, and what takes them away is the field's own decay eroding the
    // coverage under the noise, which is how foam leaves in this shader
    // everywhere else — in shrinking lace, not by dimming. The trail is the
    // only term that describes foam that is *old*.
    //
    // **The ratio is a SHARE, not an age, and using it as one greyed nine
    // tenths of a sea.** `WATER_FOAM_OLD_ALBEDO` is Koepke's drained foam,
    // ten seconds after the break; the share above reaches 1.0 the instant the
    // crest moves on, which on this trail is **0.33 s**. Measured on
    // `lucent --sim 300`: `foamAge` was 0.88 in the mean with **89.8% of the
    // water past 0.9**, so almost the whole sea was painted `(0.28, 0.30,
    // 0.32)` at 0.55 opacity — darker than the water it replaced. Setting it
    // to zero moves legible foam from 1.60% of the frame to 16.15%.
    //
    // **The trail's own decay envelope is the clock, and it is free.** Tap `k`
    // is worth at most `FOAM_TRAIL_DECAY^k`, so `log_D(foamHist)` *is* `k` and
    // `k * FOAM_TRAIL_STEP` is the seconds. Clamped to the window the trail can
    // see: a value below `D^TAPS` came from a weak break rather than an old
    // one, and reading that as age is the same mistake one level down.
    //
    // A partly-covered tap therefore reads OLDER than it is — the decay and
    // the coverage are multiplied together and only their product survives.
    // That error is in the direction the shader already had, which is why it is
    // acceptable and why it is stated here instead of being carried by a
    // second varying.
    //
    // **The consequence, said plainly: this trail cannot reach old foam.**
    // 3.3 s of window against a 10 s curve caps `foamAge` at 0.33, so
    // `WATER_FOAM_OLD_ALBEDO`'s far end is unreachable through the whitecap
    // trail and is approached only a third of the way. Foam that really is ten
    // seconds old is not in this shader's reach at all; it would need the
    // advected field to carry a deposit time, which it does not.
    float trailTaps = clamp(log(max(in.foamHist, 1.0e-4)) / log(FOAM_TRAIL_DECAY),
                            0.0, float(FOAM_TRAIL_TAPS));
    float foamAge = in.foamHist / max(max(instant, in.foamHist), 1.0e-5)
                  * saturate(trailTaps * FOAM_TRAIL_STEP / WATER_FOAM_AGE_SECONDS);
    // **Eroded by the noise rather than faded by it.** A multiply would dim
    // every foam pixel toward the water; thresholding the *coverage* against
    // the noise instead dissolves the patch from its edges inward, which is
    // how foam actually leaves — in shrinking lace, not in a uniform fade.
    float foam = smoothstep(fnoise - ferode, fnoise + ferode, wetc);
    // Past the breaking threshold foam wins outright, noise or no noise: a
    // crest that has broken is white along its whole length.
    // **Past the breaking threshold foam wins — but not uniformly.** This used
    // to be a bare `smoothstep` on the fold, which paints a solid white region
    // with a crisp outline wherever the sea is steep enough, and that flat
    // white is what reads as poured paint rather than as bubbles. A broken
    // crest is still made of bubbles, so the same noise erodes it — it simply
    // erodes far less. The narrow band is what keeps a genuinely breaking
    // crest continuous along its length instead of dissolving into speckle.
    // **Swash: the foam that runs up a beach and slides back.**
    //
    // Whitecap foam is made by a crest steepening past breaking; this is a
    // different phenomenon entirely — water that has already broken, thinned
    // out over the sand, and is being carried up the slope by its own
    // momentum. It is keyed on DEPTH, not on fold, which is why it appears
    // exactly where the sea gets shallow and nowhere else.
    //
    // The oscillation is what makes it swash rather than a painted rim: the
    // effective waterline advances and retreats, so the band sweeps up the
    // beach and drains back. A sine is enough — the real thing is the sum of
    // the same swell already in the wave list, and a second authored period
    // would be a second opinion about the same water.
    float swash = sin(push.environment[0].weather.z * SWASH_RATE) * SWASH_REACH;
    float shoreDepth = in.depth - swash;
    float shoreFoam = saturate(1.0 - shoreDepth / SHORE_FOAM_DEPTH);
    // Eroded by the same noise as the whitecaps, so the leading edge is lacy
    // rather than a contour — a clean curve is the synthetic tell, the same
    // lesson the grass slope boundary learned.
    shoreFoam = smoothstep(fnoise * 0.5, fnoise * 0.5 + 0.5, shoreFoam * shoreFoam);
    // **Capped, because swash is a thin SHEET and not a painted band.** Water
    // running up a beach is a couple of centimetres deep: you see the wet sand
    // straight through it, with foam gathered in streaks on top rather than a
    // solid white rim. Letting this reach 1.0 replaces the surface entirely
    // and throws away the refraction that is doing the most work here.
    //
    // Densest at the leading edge, where the sheet is thinnest and the air is
    // actually entrained, and thinning back toward the water — which is the
    // opposite of what a depth ramp does on its own, so it is worth the
    // second term.
    // **The waterline band: foam wherever a solid comes through the surface.**
    //
    // `gap` is the metres between this water pixel and the opaque thing behind
    // it, already reconstructed above for the refraction offset. Where a hull
    // sits just under the surface the gap goes to zero along the exact curve
    // where the boat meets the sea — which is the line a wake is drawn *from*,
    // and the one thing every water scene in this repository was missing.
    //
    // Squared before the smoothstep so the band is thin and hugs the hull
    // rather than fogging out from it, and eroded by the same `fnoise`
    // `shoreFoam` uses for the same stated reason: a clean curve is the
    // synthetic tell.
    if (gap < WATER_NO_BED) {
        float contact = saturate(1.0 - gap / WATER_CONTACT_BAND);
        contact = smoothstep(fnoise * 0.5, fnoise * 0.5 + 0.5, contact * contact);
        foam = max(foam, contact * WATER_CONTACT_OPACITY);
    }

    foam = max(foam, shoreFoam * SWASH_OPACITY);

    float broken = smoothstep(WATER_FOAM_BREAK + breakUp, WATER_FOAM_BROKEN + breakUp, in.breakMu);
    foam = max(foam, broken * smoothstep(fnoise * 0.4, fnoise * 0.4 + 0.6, broken));

    // **A material swap, not white paint.** Foam is opaque packed bubbles: it
    // has no Fresnel and no specular lobe, and painting white *over* the sky
    // reflection would leave a mirror shining through the surf. Replacing the
    // shaded result is what makes it read as a different substance.
    //
    // **Two substances, a few seconds apart.** A fresh raft is bright and
    // replaces the surface; an old one is grey and lets the water back through,
    // which is `foamAge` on both the albedo and the substitution. Swash is
    // exempt by construction — it takes `foamAge` from whatever the whitecap
    // terms said, and on a beach that is zero, so the sheet stays fresh.
    // **The raft is holes.** See `WATER_FOAM_HOLES`: without this the coverage
    // saturates and a whitecap is a flat opaque polygon with a straight edge.
    // Above the ablation, so `LOOM_ABLATE=water_foam` still zeroes everything.
    float holes = lerp(WATER_FOAM_HOLE_MEAN,
                       saturate((raft - 0.5) / WATER_FOAM_HOLE_BAND),
                       fw.x);
    foam *= 1.0 - WATER_FOAM_HOLES * holes;

    // Ablation: zero the coverage at its single point of use rather than at each of the
    // terms feeding it, so what is measured is exactly "the foam this frame drew" and
    // not one contributor to it.
    if (ablated(LOOM_ABLATE_WATER_FOAM)) { foam = 0.0; }

    // **A rough dielectric, not a Lambert card** — see `WATER_FOAM_ROUGHNESS`.
    // Measured: dropping this lobe moves **6.364% of `whitecaps`** at
    // worst-channel 38, so it is not decoration; a sunlit raft is the case it
    // is for, and the reported shot's overcast sea barely notices it (0.071%).
    float3 foamLit = lerp(WATER_FOAM_ALBEDO, WATER_FOAM_OLD_ALBEDO, foamAge)
        * (hemisphereAmbient(normal) * ambientStrength() + sunColor() * sunStrength() * toSun)
        + specular(normal, view, sunDirection(), WATER_FOAM_ROUGHNESS, float3(WATER_FOAM_F0))
          * sunColor() * sunStrength() * toSun;
    lit = lerp(lit, foamLit, foam * lerp(1.0, WATER_FOAM_OLD_OPACITY, foamAge));

    float3 fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    return float4(lerp(lit, fogColor(-view), fog), 1.0);
}

// ---------------------------------------------------------------------------
// Rain.
//
// **Stateful, as of ADR 0015.** A drop has a position and a velocity, both
// living in a GPU buffer that `rain_sim.slang` advances one fixed tick at a
// time. This file only draws it. Everything that decides where a drop *is* —
// the fall, the wind, the collision against the world, the reissue — is in the
// compute shader, and every constant both halves need is in
// `include/rain.slang`.
//
// **What was here before, and why it went.** A drop's position used to be a
// closed form of its index and the clock: a jittered world grid in the frame
// that falls with the rain, occluded per drop by the baked terrain height
// field. That was cheap, entirely stateless, and had exactly two things wrong
// with it, both named in ADR 0014's trigger list:
//
//   * a height field cannot express an overhang and knows nothing about
//     meshes, so a bridge, a gantry or a mesh roof stopped no rain at all; and
//   * splashes were spawned where sky exposure said a drop *should* have
//     landed, not where one did.
//
// Neither is fixable without state. Both are fixed by having it.
//
// **Six vertices per drop from `SV_VertexID`**, as before, and no vertex input
// at all — the same model the grass, the particles and the water use.
// ---------------------------------------------------------------------------

/// Metres of geometry between "behind the wall" and "fully visible". The soft
/// depth fade, and the reason a streak does not end in a hard line where it
/// crosses the ground.
static const float RAIN_DEPTH_FADE = 0.7;

/// The scene's depth, after the forward pass. Set 2, binding 0.
///
/// **Sampled, not tested.** Rain draws in a pass of its own, into the resolved
/// colour target, with no depth attachment at all — so the depth buffer is an
/// ordinary sampled image here. The multisampled path resolves its depth into
/// this same single-sample image, so both paths read one texel per pixel. The
/// comparison being arithmetic rather than a hardware test is what buys the
/// soft fade: a depth test can only accept or reject.
[[vk::binding(0, 2)]]
uniform Sampler2D sceneDepth;

struct RainOutput {
    float4 position : SV_Position;
    /// x across the streak in -1..1, y along it.
    float2 local : TEXCOORD0;
    float3 worldPos : TEXCOORD1;
    /// Rate, boundary fade, spawn fade and presence multiplied together.
    /// Constant over a drop, so it is decided once in the vertex stage.
    nointerpolation float strength : TEXCOORD2;
};

[shader("vertex")]
RainOutput rainVertexMain(uint vertexID: SV_VertexID) {
    uint index = vertexID / 6;
    uint corner = vertexID % 6;
    const float2 CORNERS[6] = {
        float2(-1, -1), float2(1, -1), float2(1, 1),
        float2(-1, -1), float2(1, 1), float2(-1, 1),
    };
    float2 local = CORNERS[corner];

    float3 eye = push.environment[0].eye.xyz;
    float rate = push.environment[0].rain.w;

    RainDrop drop = push.environment[0].rainDrops[index];
    float3 pos = drop.position.xyz;
    float3 velocity = drop.velocity.xyz;
    uint seed = asuint(drop.velocity.w);

    // **Four fades, and each covers a different discontinuity.**
    //
    // `edge` is the block boundary, where a drop wraps from the trailing face
    // to the leading one as the camera advances. It has to reach zero inside
    // the outermost eighth, because that is where both ends of the wrap land.
    //
    // `spawn` is new with state and covers the one thing wrapping cannot: a
    // drop that *landed* is reissued at the top of the block from wherever the
    // world stopped it, which is nowhere in particular. Without this, a drop
    // hitting a roof would blink out at the roof and blink in at the ceiling of
    // the block, and at ~550 landings a tick that is a constant sparkle.
    //
    // `present` is the rate: light rain is sparser as well as fainter, and a
    // drop keeps or drops itself against the ratio rather than the draw being
    // shortened, which would delete a contiguous slab of the buffer.
    // `near` is the fourth, and the one the other three cannot express: a drop
    // a hand's width from the eye is nowhere near a face of the block and has
    // been alive for seconds, so nothing else attenuates it — and at a constant
    // world width it draws as a bright band across a third of the frame.
    float edge = rainEdgeFade(pos, eye);
    float spawn = smoothstep(0.0, RAIN_SPAWN_FADE, drop.position.w);
    float near = rainNearFade(pos, eye);
    float present = rainPresent(seed, rate) ? 1.0 : 0.0;
    // **The fifth: is there cloud above this drop.** Per drop rather than per
    // camera, for the reason the shelter test is per drop — cover over the eye
    // deciding whether it rains across the bay is the same bug in a new
    // costume. A solid deck short-circuits to 1, so a scene that authors no
    // cover pays nothing.
    float overhead = cloudCoverAt(pos);
    float strength =
        saturate(rate / RAIN_FULL_RATE) * edge * spawn * near * present * overhead;

    // The streak: a ribbon along the direction of travel, turned to face the
    // eye. **Along this drop's own velocity**, which is what makes the layer
    // read as weather rather than as one object translating — a drop that has
    // just been caught by a gust leans differently from one that has been in it
    // for a second, and a drop falls at its own terminal speed.
    //
    // A drop with no strength collapses to a point and its triangles are
    // discarded before rasterisation — the same trick the grass fade and the
    // water LOD both use, and the reason there is no branch here.
    // **Relative to the camera, because a streak is a shutter smear.** A
    // photograph records where a drop moved across the frame, and walking
    // forward moves the frame. With only the drop's own velocity here the whole
    // field slid rigidly under a moving camera instead of stretching, which is
    // the third motion cause ADR 0017 named and the one it left untouched.
    //
    // `eyeStep` is metres per `RAIN_TICK`, so the timestep is defined once, in
    // this file. A camera falling alongside the rain sees no relative motion and
    // the streaks collapse to points, which is correct and is why the length
    // below reads the relative speed rather than the drop's.
    float3 eyeVelocity = push.environment[0].eyeStep.xyz / RAIN_TICK;
    float3 relative = velocity - eyeVelocity;
    float speed = max(length(relative), 1e-3);
    float3 along = relative / speed;
    float3 side = cross(along, eye - pos);
    float sideLength = length(side);
    // Looking straight down a streak the cross product vanishes and the ribbon
    // would collapse to a line. Any perpendicular will do there — the quad is
    // end-on and covers almost nothing.
    side = sideLength > 1e-4 ? side / sideLength : float3(1, 0, 0);

    float alive = strength > 0.0 ? 1.0 : 0.0;

    // **The screen-space width floor.** Measured from the drop's own width at
    // its own depth, exactly as the grass version does: project two points a
    // streak-width apart and read the distance in pixels. Doing it from the
    // matrix rather than from a field-of-view constant is what makes it exact
    // and indifferent to how the projection is laid out.
    //
    // Widening is paid for in `strength` below, so the light a streak adds is
    // conserved and only its distribution changes.
    float widen = 1.0;
    if (RAIN_MIN_PIXELS > 0.0) {
        float4x4 viewProj = push.objects[push.objectOffset].mvp;
        float4 clipA = mul(viewProj, float4(pos, 1.0));
        float4 clipB = mul(viewProj, float4(pos + side * RAIN_WIDTH, 1.0));
        float2 ndcA = clipA.xy / max(clipA.w, 1e-4);
        float2 ndcB = clipB.xy / max(clipB.w, 1e-4);
        float pixelsWide =
            length((ndcB - ndcA) * push.environment[0].viewport.xy * 0.5);
        if (pixelsWide > 1e-4) {
            widen = max(1.0, RAIN_MIN_PIXELS / pixelsWide);
        }
    }

    float halfLength = speed * RAIN_SHUTTER * 0.5 * alive;
    float halfWidth = RAIN_WIDTH * 0.5 * alive * widen;
    // Area went up by `widen`, so brightness comes down by it. Without this a
    // streak gets *brighter* the further away it is, which is the failure mode
    // that makes a naive width clamp read as a field of glowing scratches.
    strength /= widen;
    float3 world = pos + along * (local.y * halfLength) + side * (local.x * halfWidth);

    RainOutput out;
    // `mvp` of the reserved object slot is the view-projection — the same slot
    // the particles, the grass and the water read.
    out.position = mul(push.objects[push.objectOffset].mvp, float4(world, 1.0));
    out.local = local;
    out.worldPos = world;
    out.strength = strength;
    return out;
}

/// How much of a rain fragment survives the geometry in front of it.
///
/// **The soft depth fade.** The scene's depth is sampled rather than tested, so
/// the comparison is a ramp instead of a cliff — which is what stops a streak
/// ending in a hard line where it crosses the ground, and what a hardware depth
/// test cannot express.
///
/// Both distances are in metres from the eye: reconstructing the scene point
/// through `invViewProj` puts them on the same scale, where a difference of
/// nonlinear depths would mean something different at every range. A pixel of
/// sky reads depth 1.0 and reconstructs at the far plane, so nothing fades
/// against the background.
///
/// **Shared by the streaks and the splashes**, and it was not: this pass has no
/// depth attachment at all, so a shader that skips this is a shader that draws
/// through walls. The splashes did, and the underside of a gantry deck came out
/// speckled with the rings landing on its top.
float rainDepthFade(float2 fragment, float3 worldPos) {
    float2 uv = fragment / push.environment[0].viewport.xy;
    float depth = sceneDepth.SampleLevel(uv, 0.0).r;
    float4 unprojected = mul(push.invViewProj, float4(uv * 2.0 - 1.0, depth, 1.0));
    float3 eye = push.environment[0].eye.xyz;
    float behind = distance(eye, unprojected.xyz / unprojected.w) - distance(eye, worldPos);
    return saturate(behind / RAIN_DEPTH_FADE);
}

[shader("fragment")]
float4 rainFragmentMain(RainOutput in) : SV_Target {
    // Soft across the width and tapered at both ends, so a streak reads as a
    // smear rather than a lozenge. Additive blending has no sort and no order
    // to get wrong, which is the whole reason it was chosen: overdraw is the
    // cost of rain, and this is the version of it that needs no sorting.
    float across = 1.0 - abs(in.local.x);
    float along = 1.0 - in.local.y * in.local.y;
    float alpha = in.strength * smoothstep(0.0, 1.0, across) * along
                * RAIN_STREAK_BRIGHTNESS;

    alpha *= rainDepthFade(in.position.xy, in.worldPos);

    if (alpha <= 0.002) {
        discard;
    }

    // Rain is lit by the sky and by nothing else, so its colour is the sky's.
    // A scene authored dark gets dark rain rather than white scratches, which
    // is the rule `Environment` already imposes on everything else.
    float3 sky = lerp(skyHorizon(), skyZenith(), 0.5);
    float3 fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    // Fog attenuates an emitter rather than tinting it — the same reasoning as
    // the additive particles: blending toward the fog colour would ADD grey
    // light, and distant rain would glow through the haze.
    float3 rgb = sky * (1.0 - fog);
    // Premultiplied with alpha zeroed: the blend adds this and occludes
    // nothing behind it.
    return float4(rgb * alpha, 0.0);
}

// ---------------------------------------------------------------------------
// Splashes — where a drop actually hit.
//
// **ADR 0014's trigger 3.** These used to be CPU particles spawned from sky
// exposure: a position the CPU believed a drop *should* have reached, at a rate
// derived from how open the column was. That is a good approximation of where
// rain lands on terrain and a wrong one everywhere else — under a mesh, on a
// sloped face, in the lee of anything. Here each one is a collision the
// simulation actually resolved, with the surface normal it hit.
//
// **Drawn indirectly**, because the count is a GPU fact. `rainSplashArgsMain`
// writes the `VkDrawIndirectCommand` from the ring's cursor, so a scene where
// nothing has landed yet draws nothing rather than a buffer of stale entries
// the fragment shader has to discard one by one.
// ---------------------------------------------------------------------------

struct SplashOutput {
    float4 position : SV_Position;
    /// Position within the quad, -1..1 on both axes.
    float2 local : TEXCOORD0;
    float3 worldPos : TEXCOORD1;
    /// 0 at the instant of impact, 1 when the splash has finished.
    nointerpolation float age : TEXCOORD2;
};

[shader("vertex")]
SplashOutput rainSplashVertexMain(uint vertexID: SV_VertexID) {
    uint index = vertexID / 6;
    uint corner = vertexID % 6;
    const float2 CORNERS[6] = {
        float2(-1, -1), float2(1, -1), float2(1, 1),
        float2(-1, -1), float2(1, 1), float2(-1, 1),
    };
    float2 local = CORNERS[corner];

    RainSplash splash = push.environment[0].rainSplashes[index];
    float now = push.environment[0].weather.z;
    float age = saturate((now - splash.impact.w) / RAIN_SPLASH_LIFE);

    // An expired entry — or one from a *future* time, which is what the ring
    // holds after a headless render seeds and catches up — collapses to a point
    // and is discarded before rasterisation.
    float alive = (age < 1.0 && now >= splash.impact.w) ? 1.0 : 0.0;

    // A disc lying on the surface it hit, not a camera-facing quad: this is a
    // ring expanding across a floor, and billboarding it would make it stand up
    // as the camera lowers. The same reasoning the wetness ripples follow.
    float3 normal = splash.surface.xyz;
    float3 tangent = abs(normal.y) < 0.9 ? normalize(cross(normal, float3(0, 1, 0)))
                                         : float3(1, 0, 0);
    float3 bitangent = cross(normal, tangent);
    // **From 40% to full, not from nothing.** A ring whose radius is `age` and
    // whose brightness is `1 - age` is brightest when it is smallest, which is
    // to say invisible: it was, on the first version of this shader.
    float radius = RAIN_SPLASH_RADIUS * (0.4 + 0.6 * age) * alive;
    // Lifted a millimetre off the surface, so it is not z-fighting the floor it
    // is drawn on.
    float3 world = splash.impact.xyz + normal * 0.002
                 + tangent * (local.x * radius) + bitangent * (local.y * radius);

    SplashOutput out;
    out.position = mul(push.objects[push.objectOffset].mvp, float4(world, 1.0));
    out.local = local;
    out.worldPos = world;
    out.age = age;
    return out;
}

[shader("fragment")]
float4 rainSplashFragmentMain(SplashOutput in) : SV_Target {
    // **A crown and a ring, which is what an impact actually looks like.** The
    // spray goes up at the instant of contact and is gone in a few hundredths
    // of a second; the ring it leaves spreads and thins. Two terms rather than
    // one because a ring alone reads as a ripple on standing water, and a blob
    // alone reads as a speck of dirt.
    float r = length(in.local);
    float ring = exp(-pow((r - 1.0) * 4.0, 2.0));
    float crown = exp(-pow(r * 2.5, 2.0)) * exp(-in.age * 6.0);
    float fade = 1.0 - in.age;
    float alpha = (ring + crown) * fade * sqrt(fade) * SPLASH_BRIGHTNESS;

    // **The same soft depth fade the streaks get, and leaving it out was a
    // visible bug rather than a subtlety.** This pass has no depth attachment
    // at all — that is what buys the ramp instead of a cliff — so nothing
    // occludes a splash unless this does. Without it, the impacts on top of a
    // gantry deck drew straight through the deck and the underside of the
    // structure came out speckled with rings.
    alpha *= rainDepthFade(in.position.xy, in.worldPos);

    // Lit by the sky, like the streaks, and for the same reason: a scene
    // authored dark gets dark splashes rather than white specks.
    float3 sky = lerp(skyHorizon(), skyZenith(), 0.5);
    float3 fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    if (alpha <= 0.002) {
        discard;
    }
    return float4(sky * (1.0 - fog) * alpha, 0.0);
}
