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
float cloudCoverAt(float3 world) {
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
    return clouds_at(float3(world.x, 0.0, world.z), push.environment[0].weather.z, params).x;
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

/// What the water looks like in a direction with nothing in that direction.
///
/// Scaled by how much light is actually up there, so a night dive is dark and
/// an authored-dark environment stays dark. Brightness rather than colour is
/// taken from the sky — multiplying tint by sky *colour* would drag the
/// blue-green back toward whatever hue the sky is, which is the one thing this
/// is meant not to do.
float3 underwaterColor() {
    float above = ambientStrength()
                * dot(skyGradient(float3(0.0, 1.0, 0.0)), float3(0.2126, 0.7152, 0.0722))
                + sunStrength() * 0.25;
    return UNDERWATER_TINT * above;
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
        return underwaterColor();
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
float wetGate(float3 worldPos, float3 geometric) {
    // Off the grid, and in a scene with no terrain, this is a large negative
    // sentinel and nothing is ever under cover.
    float ground = loom_ground_height(terrainField(), worldPos.xz);
    float open = worldPos.y > ground - WET_COVER_SLACK ? 1.0 : 0.0;
    float facing = smoothstep(-0.3, 0.0, geometric.y)
        * lerp(WET_WALL, 1.0, saturate(geometric.y));
    return open * facing;
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
    }

    // The unmapped normal. Up here because the wet block below needs it, and
    // because that block now *changes the normal* — see the note on the sun
    // term underneath it.
    float3 geometric = normalize(in.normal);

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
        float pool = puddleMask(in.worldPos) * wet * wetFilm();
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
    float3 ambient = hemisphereAmbient(normal) * ambientStrength();
    // Only trace where the sun could reach anyway: a face turned away from the
    // sun is already dark, and tracing from it costs a ray to learn nothing.
    //
    // Traced against the *geometric* normal, not the mapped one: a normal map
    // can tilt a surface far enough to push the ray origin below the real
    // surface, and the ray then starts inside its own geometry.
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
    // **Submerged, the background is water, not sky.** Looking sideways with
    // no geometry in view, this pass used to paint a horizon and a sun glow
    // from the bottom of a lake. There is no direction to compute: the medium
    // is uniform, and the surface overhead is drawn by the water pass in front
    // of this one.
    if (eyeUnderwater()) {
        return float4(underwaterColor(), 1.0);
    }

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
/// Water's refractive index against air's 1.0.
///
/// It is here for one number: `asin(1/1.333)` is 48.6°, the critical angle, and
/// that cone is Snell's window.
static const float WATER_IOR = 1.333;

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
        }
        // What the underside reflects is the water itself — and since the
        // medium is one colour in every direction, the mirror comes out
        // *identical to the background*. **That is a real limitation, not an
        // oversight**: a real mirrored surface carries a reflection of the
        // seabed, and this one is featureless everywhere outside the window.
        // Fixing it needs the geometry behind the surface, which is the
        // refraction/reflection pass this step is explicitly not building.
        float3 lit = lerp(through, underwaterColor(), fresnel);
        float fogUnder = fogAmount(push.environment[0].eye.xyz, in.worldPos);
        return float4(lerp(lit, fogColor(-view), fogUnder), 1.0);
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
    float alpha = in.strength * smoothstep(0.0, 1.0, across) * along;

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
