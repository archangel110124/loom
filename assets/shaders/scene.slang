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
    uint4 material;
};

static const uint NO_TEXTURE = 0xFFFFFFFF;
static const uint FLAG_TRIPLANAR = 1;

// Mirrors `loom_render::MaterialData` exactly. One memory layout described
// twice; a mismatch is silent and looks like a shading bug.
struct MaterialData {
    float4 albedo;   // rgb base colour
    float4 params;   // metallic, roughness, uvScale.x, uvScale.y
    uint4 maps;      // albedo index, normal index, flags, unused
};

struct ParticleInstance {
    // xyz world, w radius. A NEGATIVE radius marks an additive particle —
    // fire, a spark, anything that emits light rather than blocking it. A
    // radius is never legitimately negative, so the sign is free to carry one
    // bit and the instance stays 32 bytes.
    float4 position;
    float4 color;      // rgb linear, a opacity
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
    float4 viewport;   // xy size in pixels
    float4 wind;       // xy direction, z speed, w gustiness
    float4 weather;    // x turbulence, y ground drag, z seconds, w unused
    // x still-water level, y still-water depth, z 1 when the scene has water,
    // w unused. **The depth is a constant until W6** — the terrain query that
    // makes it real is a voxel-SDF lookup this shader cannot do.
    float4 water;
    // **The sea itself, in the environment buffer rather than behind another
    // pointer.** The push block is at 116 of the 128 bytes Vulkan guarantees;
    // this is per-scene data read once per vertex, which is exactly what this
    // buffer is for. Last in the struct, so every offset above it is unmoved.
    LoomWaveSet waves;
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
    // This frame's grass blades. The block is at 124 of its 128 bytes with
    // this, which is why the wind parameters live in the environment buffer.
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

[[vk::push_constant]] Push push;

// The sky, the sun and the haze. Read from the buffer rather than baked in as
// constants, so a scene can be at night. Accessors rather than globals because
// `push` is not in scope until here.
float3 sunDirection() { return normalize(push.environment[0].sun.xyz); }
float sunStrength() { return push.environment[0].sun.w; }
float3 sunColor() { return push.environment[0].sunColor.rgb; }
float ambientStrength() { return push.environment[0].sunColor.a; }
float3 skyZenith() { return push.environment[0].zenith.rgb; }
float3 skyHorizon() { return push.environment[0].horizon.rgb; }
float fogDensity() { return push.environment[0].zenith.a; }
float fogFalloff() { return push.environment[0].horizon.a; }


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
float3 skyColor(float3 dir) {
    float3 base = skyGradient(dir);
    float toSun = max(dot(dir, sunDirection()), 0.0);
    // A tight disc inside a wide glow. Both are additive and modest — a sun
    // that blows out to white makes every screenshot harder to read, which is
    // the opposite of what this render path is for.
    float glow = pow(toSun, 8.0) * 0.18;
    float disc = pow(toSun, 512.0) * 0.9;
    return base + float3(1.0, 0.94, 0.82) * (glow + disc);
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
float fogAmount(float3 from, float3 to) {
    float3 delta = to - from;
    float distance = length(delta);
    if (distance < 1e-4) {
        return 0.0;
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
    return 1.0 - exp(-depth);
}

/// What fog looks like along a view direction.
///
/// The sun term is why fog reads as weather rather than as a wash: looking
/// into the sun through mist is bright, looking away from it is not, and that
/// asymmetry is most of what makes it convincing.
float3 fogColor(float3 direction) {
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

[shader("vertex")]
VSOutput vertexMain(uint vertexID: SV_VertexID, uint instanceID: SV_InstanceID) {
    Vertex v = push.vertices[vertexID];
    ObjectData o = push.objects[push.objectOffset + instanceID];

    float3x3 normalMatrix = float3x3(
        o.normalRow0.xyz, o.normalRow1.xyz, o.normalRow2.xyz);

    VSOutput out;
    out.position = mul(o.mvp, float4(unpackPosition(v.packedPosition, o.unpack), 1.0));
    out.normal = normalize(mul(normalMatrix, unpackNormal(v.packedNormal)));
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
    out.worldPos = mul(o.model, float4(unpackPosition(v.packedPosition, o.unpack), 1.0)).xyz;
    out.uv = unpackUv(v.packedUv, o.uvUnpack);
    return out;
}

// The scene, as the ray tracer sees it. Set 0, binding 0 — the renderer's only
// descriptor, because an acceleration structure has no buffer-device-address
// form to reach it by.
[[vk::binding(0, 0)]]
uniform RaytracingAccelerationStructure sceneTLAS;

/// 1 where the sun reaches this point, 0 where something blocks it.
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
float sunVisibility(float3 position, float3 normal, float bias) {
    RayDesc ray;
    // Offset along the normal, not just by TMin: a ray starting exactly on the
    // surface hits the surface it came from, and the whole scene self-shadows
    // into acne.
    ray.Origin = position + normal * bias;
    ray.Direction = sunDirection();
    ray.TMin = bias;
    ray.TMax = 1e4;

    RayQuery<RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH
           | RAY_FLAG_SKIP_PROCEDURAL_PRIMITIVES> query;
    query.TraceRayInline(sceneTLAS, RAY_FLAG_NONE, 0xFF, ray);
    query.Proceed();
    return query.CommittedStatus() == COMMITTED_TRIANGLE_HIT ? 0.0 : 1.0;
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
    return sampleMap(index, position.yz) * weight.x
         + sampleMap(index, position.xz) * weight.y
         + sampleMap(index, position.xy) * weight.z;
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

    float3 nx = sampleMap(index, position.yz).xyz * 2.0 - 1.0;
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

    if (in.material != NO_TEXTURE) {
        MaterialData m = push.materials[in.material];
        albedo = m.albedo.rgb;
        metallic = m.params.x;
        roughness = m.params.y;

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
    }

    float diffuse = max(dot(normal, sunDirection()), 0.0);
    float3 ambient = hemisphereAmbient(normal) * ambientStrength();
    // Only trace where the sun could reach anyway: a face turned away from the
    // sun is already dark, and tracing from it costs a ray to learn nothing.
    //
    // Traced against the *geometric* normal, not the mapped one: a normal map
    // can tilt a surface far enough to push the ray origin below the real
    // surface, and the ray then starts inside its own geometry.
    float3 geometric = normalize(in.normal);
    float sun = diffuse > 0.0 ? sunVisibility(in.worldPos, geometric, in.shadowBias) : 0.0;

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
    float3 environment = skyGradient(normalize(lerp(reflection, normal, roughness)));
    lit += environment * f0 * (1.0 - roughness * 0.7);

    // Last, over the finished shading. Fog is what is between the eye and the
    // surface, so it replaces the surface's colour rather than joining the
    // lighting that produced it.
    float fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    lit = lerp(lit, fogColor(-view), fog);

    return float4(lit, 1.0);
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

    float3 world = p.position.xyz + (right * spun.x + up * spun.y) * abs(p.position.w);

    ParticleOutput out;
    // `mvp` of the reserved object slot is the view-projection — see the note
    // on `Push::object_offset` in the Rust mirror.
    out.position = mul(push.objects[push.objectOffset].mvp, float4(world, 1.0));
    out.local = local;
    out.color = p.color;
    out.worldPos = world;
    out.seed = float(index);
    out.additive = p.position.w < 0.0 ? 1.0 : 0.0;
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
[shader("fragment")]
float4 particleFragmentMain(ParticleOutput in) : SV_Target {
    float r = length(in.local);
    if (r > 1.0) {
        discard;
    }

    // Two octaves of value noise in the quad's own space, offset per particle
    // so neighbours do not share a pattern.
    float2 q = in.local * 1.7 + in.seed * 7.31;
    float wisp = 0.62 * valueNoise(q) + 0.38 * valueNoise(q * 2.4 + 3.7);

    // Falloff from the centre, eaten into by the noise. `smoothstep` rather
    // than a linear ramp: a linear edge on a translucent sprite reads as a
    // visible disc boundary once several overlap.
    float falloff = smoothstep(1.0, 0.15, r);
    float alpha = in.color.a * falloff * (0.45 + 0.85 * wisp);
    if (alpha <= 0.002) {
        discard;
    }
    float3 toEye = push.environment[0].eye.xyz - in.worldPos;
    float fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
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

    float3 rgb = lerp(in.color.rgb, fogColor(-normalize(toEye)), fog);
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
    float fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
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
// vertex shader is already deciding for free. Seven levels reach 512 m.
//
// **The grid is snapped to the COARSEST cell, so no vertex ever slides.** A
// camera-centred mesh that follows the eye continuously moves every vertex
// every frame, which changes every vertex's displacement every frame, which is
// a shimmer generator. Snapped, the vertices sit on a fixed world lattice and
// the mesh is a window onto it: the eye can move and nothing in the geometry
// moves at all. All levels share one snap because they must share a centre for
// the holes to line up, and the coarsest cell is the only increment every
// level's lattice divides. The cost is that the window jumps 32 m at a time —
// which moves no vertex, only the LOD boundaries, and those are far away by
// construction.
//
// **No cracks, by construction rather than by skirt.** Each level's outermost
// vertex ring is snapped to the next level's lattice, so the two share exactly
// the same vertices along their shared edge and evaluate the same function at
// them. The fine side's spare vertices collapse onto their neighbours and the
// triangles between them are degenerate, which costs nothing. A skirt would
// hide a gap; this removes it.
// ---------------------------------------------------------------------------

/// Cells per side, per level. Also the vertex count's only real lever: seven
/// levels of 32x32 quads is 43,008 vertices, which is nothing.
static const uint WATER_RES = 32;
/// How many rings. Level L has cells of `WATER_CELL * 2^L`, so the outermost
/// reaches `WATER_RES * WATER_CELL * 2^(L-1)` metres — 512 m at these numbers.
static const uint WATER_LEVELS = 7;
/// The finest cell, in metres, right under the camera.
static const float WATER_CELL = 0.5;

struct WaterOutput {
    float4 clip : SV_Position;
    float3 worldPos : TEXCOORD0;
    float3 normal : TEXCOORD1;
    /// Still-water depth under this point, in metres. Constant until W6 — see
    /// the note on `EnvironmentData::water`.
    nointerpolation float depth : TEXCOORD2;
};

[shader("vertex")]
WaterOutput waterVertexMain(uint vertexID: SV_VertexID) {
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
    float2 xz = centre
        + (float2(float(ix), float(iz)) - float(WATER_RES) * 0.5) * cellSize;

    float surface = push.environment[0].water.x;
    float depth = push.environment[0].water.y;
    LoomWaterSample surfaceAt = loom_sample_water(
        push.environment[0].waves,
        surface,
        surface - depth,
        xz,
        push.environment[0].weather.z);

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
    out.worldPos = world;
    out.normal = surfaceAt.normal;
    out.depth = surfaceAt.depth;
    return out;
}

/// What deep water looks like when you see into it rather than off it.
/// Linear, and dark on purpose — most of what a viewer reads as "the colour of
/// the sea" is the sky bouncing off it, which the Fresnel term below supplies.
static const float3 WATER_DEEP = float3(0.004, 0.018, 0.030);
static const float3 WATER_SHALLOW = float3(0.020, 0.070, 0.075);
/// Depth over which the tint reaches its deep value, in metres.
static const float WATER_TINT_DEPTH = 5.0;
/// **Not zero.** A mirror-smooth sea gives a specular lobe the size of a
/// pixel, which is the classic water shimmer; this is the smallest roughness
/// that still reads as sun glitter rather than as a hard white dot.
static const float WATER_ROUGHNESS = 0.08;

[shader("fragment")]
float4 waterFragmentMain(WaterOutput in) : SV_Target {
    float3 toEye = push.environment[0].eye.xyz - in.worldPos;
    float3 view = normalize(toEye);
    float3 normal = normalize(in.normal);
    // Seen from underneath — the far wall of a trough, and eventually W7's
    // underwater camera — the analytic normal points away. Flipping it keeps
    // the shading continuous instead of letting the surface go black.
    if (dot(normal, view) < 0.0) {
        normal = -normal;
    }

    // Schlick against water's f0. This is the whole trick: face-on you see
    // into the water, at a grazing angle you see the sky, and the crossover is
    // most of what makes a surface read as liquid.
    float fresnel = 0.02 + 0.98 * pow(saturate(1.0 - dot(normal, view)), 5.0);

    float3 body = lerp(WATER_SHALLOW, WATER_DEEP, saturate(in.depth / WATER_TINT_DEPTH));
    float toSun = max(dot(normal, sunDirection()), 0.0);
    float3 through = body
        * (hemisphereAmbient(normal) * ambientStrength() + sunColor() * sunStrength() * toSun)
        // Light scattered up through a crest, which is what makes a wave read
        // as translucent rather than as painted metal. Strongest looking into
        // the sun, which is where a real one glows.
        + body * 6.0 * sunStrength() * pow(saturate(dot(-view, sunDirection())), 3.0);

    float3 lit = lerp(through, skyColor(reflect(-view, normal)), fresnel);
    lit += specular(normal, view, sunDirection(), WATER_ROUGHNESS, float3(0.02))
         * sunColor() * sunStrength() * toSun;

    float fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    return float4(lerp(lit, fogColor(-view), fog), 1.0);
}
