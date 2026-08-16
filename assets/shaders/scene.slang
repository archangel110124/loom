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
    float4 albedo;   // rgb base colour, w porosity (how much it darkens wet)
    float4 params;   // metallic, roughness, uvScale.x, uvScale.y
    uint4 maps;      // albedo index, normal index, flags, unused
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

float3 pointLights(float3 worldPos, float3 normal) {
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
        float atten = window * window / max(d2, 1e-4);
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

/// Extinction per metre under the surface.
///
/// **The whole effect is the ratio to the fog above.** A scene authors
/// `fog_density` in the thousandths — hundreds of metres of visibility — and
/// this is 0.07: an e-fold in fourteen metres, three quarters gone by twenty.
/// Clear sea rather than soup, and a calibration knob rather than a constant of
/// nature — it is the number to turn if a lake should read as murkier than an
/// ocean, and the number a per-body `clarity` would replace when one exists.
static const float UNDERWATER_EXTINCTION = 0.07;

/// The water's own colour, before the light coming down through it.
///
/// Blue-green rather than blue: red goes first in real water, and a tint that
/// is merely a darker sky reads as night rather than as being underneath
/// something.
static const float3 UNDERWATER_TINT = float3(0.08, 0.30, 0.28);
/// How much of the surface's light still reaches you looking straight DOWN.
/// Small: below you is a deep column that has already absorbed nearly
/// everything, which is why the sea floor of a photograph fades to black long
/// before the surface does.
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
/// taken from the sky — multiplying tint by sky *colour* would drag the
/// blue-green back toward whatever hue the sky is, which is the one thing this
/// is meant not to do.
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

    return UNDERWATER_TINT * above * column
         + sunColor() * shaft * UNDERWATER_SHAFT;
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
    // **Submerged, the same term with different constants.** Water's density
    // does not fall off with altitude — the height falloff below is an
    // atmosphere model and would make the sea thinner near the surface — so
    // this is plain Beer-Lambert over the distance. One branch, taken by every
    // surface in the frame at once, which is what makes it an atmosphere
    // rather than a tint on the water shader.
    if (eyeUnderwater()) {
        return 1.0 - exp(-distance * UNDERWATER_EXTINCTION);
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
    float3 origin = float3(o.model[0][3], o.model[1][3], o.model[2][3]);
    float3x3 m3 = float3x3(o.model[0].xyz, o.model[1].xyz, o.model[2].xyz);
    modelPos = windBend(modelPos, m3, origin, o.color.a);
    out.position = mul(o.mvp, float4(modelPos, 1.0));
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
    // The *bent* position, so lighting, fog and the wetness gate all see the
    // vertex where it actually is. Recomputing the unbent one here would light
    // a tree as though it were standing still.
    out.worldPos = mul(o.model, float4(modelPos, 1.0)).xyz;
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

/// **Eight, and the reason is an occupancy cliff rather than the noise.**
///
/// Noise against a 64-ray reference on `primitives` at 640x400, and cost on
/// `lanternhead` at 1920x1080 over a 4-ray soft shadow, forward pass, median of
/// three (0.467 ms with AO stubbed out):
///
///     rays          2       4       8       16      32
///     worst         84      64      41      24      24
///     mean          .1201   .0712   .0415   .0250   .0146
///     forward ms    0.511   0.605   0.806   1.618   2.100
///
/// Rays are 0.022, 0.024 and 0.025 ms each up to eight, and then **0.101 ms
/// each** from eight to sixteen — a fourfold jump with nothing in the shader
/// changing but the trip count, which is what a register-pressure or occupancy
/// cliff looks like. Sixteen rays would cost more than the whole rest of the
/// frame to take the worst-channel error from 41 to 24, both comfortably inside
/// the gate's 72.
///
/// An AO ray is also far more expensive than a sun ray at the same count — 0.024
/// against 0.005 ms — and the reason is coherence: every pixel's sun rays go
/// nearly the same way and share their traversal, while a hemisphere sample goes
/// wherever the surface faces.
static const uint AO_RAYS = 8;

/// The fraction of the hemisphere above `normal` that is open, within
/// [`AO_RANGE`].
float ambientVisibility(float3 position, float3 normal, float bias, float2 pixel) {
    float3 origin = position + normal * bias;
    float3 u, v;
    rayBasis(normal, u, v);

    // Offset into the sequence, so a pixel's AO rays are not the same four
    // directions its shadow rays used. Sharing them correlates the two terms
    // and puts a visible dither pattern where a penumbra crosses a corner.
    float2 dither = rayDither(pixel + float2(37.0, 11.0));
    float open = 0.0;
    for (uint i = 0; i < AO_RAYS; ++i) {
        float2 xi = raySample(dither, i);
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
    return open / float(AO_RAYS);
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
float3 tracedEnvironment(float3 position, float3 normal, float3 dir,
                         float roughness, float bias) {
    float3 sky = skyGradient(dir);
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
    float3 albedo = hitObject.color.rgb;
    if (hitObject.material.x != NO_TEXTURE) {
        // The material's base colour, not its texture. Fetching a texel would
        // need the triangle's UVs, which needs the index buffer, which is one
        // more pointer than the push block has room for — and at the
        // roughnesses this runs at, a reflection is a smear of a surface rather
        // than a readable image of it.
        albedo = push.materials[hitObject.material.x].albedo.rgb;
    }

    float3 hit = origin + dir * query.CommittedRayT();
    // **The reversed ray direction, standing in for the hit's normal.**
    //
    // Exact whenever the surface faces the ray head-on and wrong by the angle
    // of incidence otherwise, so it is right at the centre of a reflection and
    // degrades at its edges — a floor reflecting a wall gets the wall's normal
    // exactly, a floor reflecting a distant roof at a grazing angle does not.
    // The alternative is the triangle's own normal, which means the index
    // buffer, a per-mesh first-index, and three vertex fetches per reflected
    // pixel; ADR 0019 records that trade and why it was not taken yet.
    float3 hitNormal = -dir;
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
    if (waterPresent()) {
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
                   * ambientVisibility(in.worldPos, geometric, in.shadowBias, in.position.xy);
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
        ? sunVisibility(in.worldPos, geometric, in.shadowBias, in.position.xy)
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
    float3 environment = tracedEnvironment(in.worldPos, geometric, lobe, roughness,
                                           in.shadowBias);
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
    // The sign of red selects the flame field. Free: the schema clamps colour
    // to [0, 1] so the bit was unused, and `abs` puts the colour back.
    out.flame = p.color.r < 0.0 ? 1.0 : 0.0;
    out.color = float4(abs(p.color.r), p.color.g, p.color.b, p.color.a);
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
// **One additive quad, and the flame is a LEVEL SET of a noise field.** Not a
// sprite, not a flipbook, not a stack of blobs.
//
// The reason is overdraw. The colour target is `R8G8B8A8_SRGB` — fixed point,
// so every additive blend clamps at 1.0 with no rolloff anywhere. Measured on
// the sprite path, red pinned after 2.6 overlapping particles against about
// thirty alive, so the middle of the fire was always going to be white however
// its shapes were authored. Summing soft sprites cannot make fire here: the
// sum of many round things is a round thing, and then it clips.
//
// A single quad has overdraw of exactly one. Nothing can clip, so the colour
// ramp is honoured everywhere, and the tongues, the black gaps between them
// and the edges are all level sets of one function — which is why the edge is
// one pixel wide at any distance instead of being a texture's bilinear ramp.
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
/// The brightness transfer, with a floor. Without the floor, brightness inside
/// the coverage edge is cubic in depth-into-the-flame, so the outermost pixels
/// read as black and full colour arrives several pixels in — the shader paints
/// the very halo it exists to remove.
static const float FIRE_TAU_FLOOR = 0.30;
static const float FIRE_TAU_DEN = 0.27;
static const float FIRE_TAU_POW = 1.6;
static const float FIRE_TAU_H = 0.42;
/// Where the finest octave is retired. `loom_value_noise` is analytic and has
/// no mip chain, so past the distance at which that octave falls under a pixel
/// it is a twinkle generator — the same bound and the same reason as
/// `WATER_DETAIL_RANGE`. Computed from `length(toEye)`, which is analytic and
/// therefore correct even in a quad the discard has killed.
static const float FIRE_RANGE = 26.0;

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

/// The flame at one fragment of the quad.
float4 flameColor(ParticleOutput in, float3 toEye) {
    // Flame-local coordinates: x across, y up from the base, z through.
    float3 w = in.worldPos - in.origin.xyz;
    float half_h = max(in.origin.w, 1e-3);
    // **The quad is centred on the emitter, so the flame's base is at its
    // BOTTOM edge, not at its middle.** Mapping `h` from `w.y / half_h` puts
    // the fire in the upper half only, floating clear of whatever it is
    // supposed to be burning — which is exactly what it did.
    float h = saturate((w.y + half_h) / (2.0 * half_h));

    float detail = saturate(1.0 - length(toEye) / FIRE_RANGE);
    float t = push.environment[0].weather.z;

    // **The warp is built BEFORE the vertical squash.** Warping afterwards
    // multiplies the warp's vertical amplitude by 1/FIRE_SQUASH — about seven
    // — along the one axis whose coherence is what makes a feature taller than
    // it is wide, and the tongues dissolve into froth.
    float3 qi = float3(w.x, w.y / (1.0 + FIRE_ACCEL * h) - FIRE_RISE * t, w.z) * FIRE_HZ;
    float w1 = fireFbm(qi + 17.1, 2, detail) - 0.5;
    float w2 = fireFbm(qi + 43.7, 2, detail) - 0.5;
    float3 warp = float3(w1 * FIRE_WARP_XZ, w2 * FIRE_WARP_Y, (w1 - w2) * FIRE_WARP_XZ);
    float3 q = qi + warp;
    q.y *= FIRE_SQUASH;
    float d = fireFbm(q, 3, detail);

    // **The envelope and the fuel OFFSET the threshold; they never multiply
    // the field.** A multiply's edge is the mask's own gradient, which is far
    // softer than the field's, and a soft-edged gap between two tongues is
    // precisely what turns a fire back into a fireball.
    float radial = length(float2(w.x, w.z)) / max(half_h * 1.15, 1e-3);
    float cone = radial * (1.0 + FIRE_CONE * h);
    // The fuel line: a few lobes across the base, so tongues start apart
    // rather than being one sheet that noise happens to tear.
    float lobes = 0.5 + 0.5 * sin(w.x * FIRE_LOBES + w.z * 0.7);
    float gap = FIRE_GAP * (1.0 - lobes) * (1.0 - h * 0.4);

    float threshold = FIRE_T0 + (FIRE_T1 - FIRE_T0) * pow(h, FIRE_T_POW)
                    + cone * 0.22 + gap;
    float e = d - threshold;

    // Coverage from the analytic screen-space width of the level set. Computed
    // before any discard: derivatives in a quad with killed lanes are
    // undefined, and that is the trap named for the water shoreline.
    float cover = saturate(e / max(fwidth(e), 1e-4));
    if (cover <= 0.0) {
        return float4(0.0, 0.0, 0.0, 0.0);
    }

    float tau = FIRE_TAU_FLOOR
              + (1.0 - FIRE_TAU_FLOOR) * pow(saturate(e / FIRE_TAU_DEN), FIRE_TAU_POW);
    tau *= (1.0 - FIRE_TAU_H * h);

    return float4(fireRamp(tau) * cover * in.color.a, 0.0);
}

[shader("fragment")]
float4 particleFragmentMain(ParticleOutput in) : SV_Target {
    float3 eyeVec = push.environment[0].eye.xyz - in.worldPos;
    if (in.flame > 0.5) {
        float4 fire = flameColor(in, eyeVec);
        if (fire.r + fire.g + fire.b <= 0.0008) {
            discard;
        }
        float fogF = fogAmount(push.environment[0].eye.xyz, in.worldPos);
        return float4(fire.rgb * (1.0 - fogF), 0.0);
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

    LoomWaterSample surfaceAt = loom_sample_water(
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
        float3(0.0, 0.0, 0.0));

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
    out.depth = min(surfaceAt.depth, 100.0);
    out.fold = surfaceAt.fold;
    return out;
}

/// Beam attenuation per metre at roughly 650 / 550 / 450 nm — Pope & Fry
/// absorption plus Morel molecular scattering. Red is gone in three metres,
/// blue survives fifty, and that spread is why deep water is blue at all.
static const float3 WATER_EXTINCT = float3(0.341, 0.058, 0.013);
/// Backscatter for pure water. **The turbidity knob, and the only one.**
/// Raising it turns navy toward teal, because what comes back up out of a
/// water column is scattered light, not reflected light.
static const float3 WATER_BACKSCATTER = float3(0.00035, 0.00075, 0.00175);

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
static const float3 WATER_DEEP =
    0.33 * WATER_BACKSCATTER / (WATER_EXTINCT + WATER_BACKSCATTER);
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
/// Metres of water a ray crosses at a crest and in a trough. Not measured
/// depths — an effective path length for the wrap lobe, and the ratio between
/// them is what carries the effect.
static const float WATER_CREST_THICK = 0.9;
static const float WATER_TROUGH_THICK = 4.0;
/// How far the sun direction is bent by the surface normal before the lobe is
/// evaluated. Widens the glow around a crest rather than pinning it to exact
/// alignment.
static const float WATER_SSS_DISTORT = 0.35;
static const float WATER_SSS_POWER = 4.0;
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

static const float WATER_FOAM_WET = 0.22;
static const float WATER_FOAM_BREAK = 0.33;
/// Cross-wind stretch of the noise domain. **This is what makes streaks** —
/// foam pulled out downwind rather than dropped in round blobs, which is the
/// most recognisable thing about a real whitecap field.
static const float WATER_FOAM_STRETCH = 6.0;
/// Not white. Foam is packed bubbles, and bubbles are dielectric films: the
/// brightest sea foam in a photograph sits well under paper white, and pinning
/// it at 1.0 is what makes rendered surf read as painted.
static const float3 WATER_FOAM_ALBEDO = float3(0.72, 0.76, 0.78);
/// Where the second octave stops.
///
/// **Not taste — an aliasing bound.** Its cross-wind lattice period is
/// `1/(WATER_FOAM_STRETCH · 2.3)` = 7.3 cm, and `loom_value_noise` is evaluated
/// analytically with no mip chain, so past ~30 m that octave is far under a
/// pixel and becomes a twinkle generator. One `saturate` retires it.
static const float WATER_FOAM_FINE_RANGE = 40.0;

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
/// **No `ddx`/`ddy` anywhere.** The shoreline `discard` is the first statement
/// of `waterFragmentMain`, so derivatives in those quads are undefined — and
/// those quads are exactly the waterline that `shore` and `river` exist to
/// protect. The distance fade is computed from `length(toEye)` instead, which
/// is an analytic footprint and correct in a killed quad.
static const float WATER_DETAIL_SCALE = 1.6;
/// How hard the ripples tilt the surface. A slope, so 0.45 is roughly a
/// 24-degree local tilt at the steepest point of a ripple — capillary waves are
/// genuinely that steep for their length, which is why they catch so much light
/// for so little amplitude.
static const float WATER_DETAIL_STRENGTH = 0.30;
/// Where the detail stops. `loom_value_noise` is analytic and has no mip chain,
/// so past this the lattice is under a pixel and becomes a twinkle generator
/// rather than detail — the same bound the foam's fine octaves take, for the
/// same reason.
static const float WATER_DETAIL_RANGE = 70.0;
float2 waterDetailSlope(float2 xz, float t, float2 drift) {
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
float causticWeb(float3 bedPos, float depth) {
    float2 cuv = bedPos.xz * CAUSTIC_SCALE;
    float2 cdrift = float2(0.07, 0.05) * push.environment[0].weather.z;
    float c1 = loom_value_noise(float3(cuv + cdrift, 0.0));
    float c2 = loom_value_noise(float3(cuv * 1.31 - cdrift * 0.83, 5.7));
    float web = pow(saturate(1.0 - abs(c1 - c2) * CAUSTIC_SHARP), CAUSTIC_POWER);
    // Strongest just under the surface and gone by a few metres — a ripple's
    // focal length is short and a deep column washes the web out entirely.
    // Gone at a low sun too, since a grazing ray never focuses.
    float fade = saturate(1.0 - depth / CAUSTIC_DEPTH)
               * saturate(sunDirection().y * 1.6);
    return web * CAUSTIC_STRENGTH * fade;
}

float3 waterBehind(float2 uv, float3 surfacePos, float3 eye, float bedDepth, out float3 behind) {
    behind = float3(0.0, 0.0, 0.0);
    float d = opaqueDepth.SampleLevel(uv, 0).r;
    if (d >= 1.0) {
        return float3(0.0, 0.0, 0.0);
    }
    // Reconstruct where that pixel is, so the path length is the real one
    // rather than an authored depth. `in.depth` is a still-water bed depth
    // clamped at 100 and is a SENTINEL on any scene with no voxel volume;
    // using it as an optical path is what would paint 27% seabed brown across
    // every open-ocean pixel.
    float2 ndc = uv * 2.0 - 1.0;
    float4 world = mul(push.invViewProj, float4(ndc, d, 1.0));
    float3 bg = world.xyz / world.w;

    // Two legs, not one. The seabed was shaded IN AIR by the mesh pass, so the
    // light that lit it already crossed the column downward before it ever
    // reflected toward the eye. Dividing by the sun's elevation is the same
    // secant the subsurface term four lines below already uses.
    float viewLeg = length(bg - surfacePos);
    float downLeg = min(bedDepth, 100.0) / max(sunDirection().y, 0.15);
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
    behind *= 1.0 + causticWeb(bg, min(bedDepth, 100.0));
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
    float bgFog = fogAmount(eye, bg);
    behind = max((behind - fogColor(normalize(bg - eye)) * bgFog) / max(1.0 - bgFog, 1e-3),
                 float3(0.0, 0.0, 0.0));

    return exp(-WATER_EXTINCT * (viewLeg + downLeg));
}

[shader("fragment")]
float4 waterFragmentMain(WaterOutput in) : SV_Target {
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
    if (in.depth <= 0.0) {
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
        float fogUnder = fogAmount(push.environment[0].eye.xyz, in.worldPos);
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
    {
        float detailFade = saturate(1.0 - length(toEye) / WATER_DETAIL_RANGE);
        // Calmer over a beach than over the shelf, which is true of real water
        // and keeps the ripples off the very edge of the shoreline cut.
        detailFade *= saturate(in.depth * 0.7 + 0.2);
        float2 dw = normalize(push.environment[0].wind.xy + float2(1e-4, 0.0));
        float2 g = waterDetailSlope(
            in.worldPos.xz,
            push.environment[0].weather.z,
            dw * max(push.environment[0].wind.z, 0.5) * 0.06);
        // A slope subtracted from the normal's horizontal components — the
        // standard perturbation. The surface stays exactly where the vertex
        // shader put it; only how it catches light changes, which is what keeps
        // this out of the "second opinion about the drawn surface" that the
        // rejected wave-set version would have been.
        normal = normalize(normal - float3(g.x, 0.0, g.y) * WATER_DETAIL_STRENGTH * detailFade);
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

    // **Backlit scatter, gated on how THIN the water is — not on view angle.**
    //
    // The term this replaces was `body * 6.0 * pow(dot(-view, sunDir), 3)`,
    // which glows a trough exactly as brightly as a crest and so reads as haze
    // laid over the whole sea rather than as light coming through a wave. A
    // real crest glows because it is a thin sheet with the sun behind it, and
    // thickness is what decides how much survives the crossing.
    //
    // Wrap-lobe form (Barré-Brisebois): perturbing the sun direction by the
    // surface normal widens the lobe so the glow spreads around the crest
    // instead of appearing only at exact alignment. **The hue is not authored**
    // — it falls out of `WATER_EXTINCT` through Beer-Lambert, so the colour of
    // a backlit wave is the colour water actually transmits: through 2 m that
    // is (0.51, 0.89, 0.97), the luminous turquoise a breaking crest has.
    //
    // Dividing thickness by the sun's elevation is why crests glow at a low sun
    // and barely at noon: a shallow sun crosses more water to reach the eye.
    float thick = lerp(WATER_TROUGH_THICK, WATER_CREST_THICK, crest)
                / max(sunDirection().y, 0.15);
    float3 h = normalize(sunDirection() + normal * WATER_SSS_DISTORT);
    float3 sss = sunColor() * sunStrength()
               * pow(saturate(dot(-view, h)), WATER_SSS_POWER)
               * crest * exp(-WATER_EXTINCT * thick);

    // **What the eye actually receives from below, split by how much water is
    // in the way.** `T` is the fraction that crossed the column; `1 - T` is
    // the fraction that scattered before it got there.
    //
    // The old form was `body * downwelling`, where `body` interpolated
    // `WATER_SHALLOW` toward `WATER_DEEP` by an authored bed depth. That was a
    // hand-picked *linear* stand-in for exactly this, with no view-angle
    // dependence, and it is deleted rather than supplemented — this is the
    // decomposition it approximated. `L -> infinity` gives `WATER_DEEP *
    // downwelling`, which is what it used to answer everywhere; `L -> 0` gives
    // the background unattenuated, which is the shoreline.
    //
    // **The partition conserves energy**, which is why adding a seabed makes
    // the water deeper rather than brighter.
    float3 downwelling = hemisphereAmbient(normal) * ambientStrength() * skyView
                       + sunColor() * sunStrength() * toSun;
    float2 screenUv = in.clip.xy / push.environment[0].viewport.xy;
    float3 behind;
    // **Unrefracted UV.** The distortion offset is the next slice; getting the
    // optics right first means one thing at a time, which is how a wrong sign
    // in an offset does not get blamed on the absorption.
    float3 T = waterBehind(screenUv, in.worldPos, push.environment[0].eye.xyz, in.depth, behind);
    // `skyView` scales the volume term ONLY. `behind` was lit by its own
    // shading pass; darkening it in a trough would dim the seabed for a reason
    // that has nothing to do with the seabed.
    float3 through = behind * T + WATER_DEEP * downwelling * (1.0 - T) + sss;

    float3 lit = lerp(through, skyColor(reflect(-view, normal)), fresnel);
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
    float2 wd = normalize(windv.xy + float2(1e-4, 0.0));
    // Foam drifts downwind. Sampling in a frame that slides with the wind is
    // what keeps a patch from sitting nailed to the world while the wave that
    // made it travels out from under it.
    float2 wp = in.worldPos.xz - wd * windv.z * push.environment[0].weather.z * 0.05;
    float2 fuv = float2(dot(wp, wd), dot(wp, float2(-wd.y, wd.x)) * WATER_FOAM_STRETCH);
    float fine = saturate(1.0 - length(toEye) / WATER_FOAM_FINE_RANGE);
    float fnoise = loom_value_noise(float3(fuv * 0.55, 0.0)) * 0.65
                 + loom_value_noise(float3(fuv * 2.3, 7.3)) * 0.35 * fine;
    float wetc = smoothstep(WATER_FOAM_WET, WATER_FOAM_BREAK, in.fold);
    // **Eroded by the noise rather than faded by it.** A multiply would dim
    // every foam pixel toward the water; thresholding the *coverage* against
    // the noise instead dissolves the patch from its edges inward, which is
    // how foam actually leaves — in shrinking lace, not in a uniform fade.
    float foam = smoothstep(fnoise - 0.15, fnoise + 0.15, wetc);
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
    foam = max(foam, shoreFoam * SWASH_OPACITY);

    float broken = smoothstep(WATER_FOAM_BREAK, 0.50, in.fold);
    foam = max(foam, broken * smoothstep(fnoise * 0.4, fnoise * 0.4 + 0.6, broken));

    // **A material swap, not white paint.** Foam is opaque packed bubbles: it
    // has no Fresnel and no specular lobe, and painting white *over* the sky
    // reflection would leave a mirror shining through the surf. Replacing the
    // shaded result is what makes it read as a different substance.
    float3 foamLit = WATER_FOAM_ALBEDO
        * (hemisphereAmbient(normal) * ambientStrength() + sunColor() * sunStrength() * toSun);
    lit = lerp(lit, foamLit, foam);

    float fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
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
    float fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
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
    float fog = fogAmount(push.environment[0].eye.xyz, in.worldPos);
    if (alpha <= 0.002) {
        discard;
    }
    return float4(sky * (1.0 - fog) * alpha, 0.0);
}
