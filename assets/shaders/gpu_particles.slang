// The GPU particle pool: one emitter's population, simulated on the device.
//
// ADR 0047, and it generalises `rain_sim.slang` (ADR 0017) rather than
// inventing anything. Read that file first — everything structural here is its
// argument applied to a population that is not fixed.
//
// # The one thing rain did not have to solve: spawning
//
// Rain never spawns. Its population is 131,072 forever, thread `i` owns drop
// `i`, and a landed drop is recycled in place. A real emitter releases
// particles over time, and the textbook answer is a free list with an atomic:
// a dying thread pushes its slot, a spawning thread pops one. **That answer is
// closed to this project.** The order the atomics resolve in picks the slot,
// the slot picks the seed, and the seed picks a position, a roll and a
// lifetime — all of which are in the golden image. A gate that depends on
// atomic ordering is a gate that flakes.
//
// So slot ownership here is **arithmetic and has no state of its own**:
//
//     slot i owns the largest birth ordinal n < birthsBy(tick) with n ≡ i (mod N)
//
// which is `((births - 1 - i) / N) * N + i`. No free list, no dead list, no
// alive list, no atomic anywhere, and no indirect dispatch — the dispatch is
// `N` threads and a slot with nothing in it writes a degenerate quad. Two
// consequences worth stating plainly:
//
// - **`birthsBy` is the allocator.** It is not bookkeeping. Its CPU twin is
//   `loom_particles::births_by`, and a test compares them tick by tick over ten
//   thousand ticks, because a disagreement is a different number of particles
//   with nothing to blame.
// - **`N` is a correctness parameter.** Ordinals `n` and `n + N` share a slot
//   and are born `N / rate` seconds apart, so a pool smaller than
//   `burst + rate * maxLifetime` overwrites live particles — the plume blinks.
//   `loom_scene` refuses that at load with the required number in the error.
//
// # No cross-thread dependency, so a catch-up is one dispatch
//
// Exactly rain's argument: thread `i` reads and writes only `pool[i]`, so
// advancing N ticks is one dispatch with the recurrence rolled up in registers
// rather than N dispatches with N barriers. `--sim 1800` costs one dispatch.
//
// # Determinism
//
// Nothing here is read back, ever (ADR 0045 clause 2). A GPU particle is
// therefore invisible to `loom sim --assert` and to `rhai`, by construction and
// not by omission — a particle a script must reason about is a CPU particle,
// which is what `loom_particles` already is.
//
// What state costs is that a frame is a function of the *sequence* of ticks the
// buffer has been advanced through rather than of the tick alone. A headless
// still seeds and advances exactly `--sim` ticks in one dispatch, which is
// reproducible run to run — and `cargo xtask repeat` is what proves it rather
// than asserting it.
//
// # This is the same model as the CPU emitter, not a bit-identical twin
//
// The force chain below is `loom_particles::System::advance` transcribed: swirl,
// gravity, wind coupling, damping, Euler. The *random* half cannot be
// transcribed — the CPU draws from a seeded `loom_terrain::noise::Rng`
// sequence, which is inherently serial, and the GPU hashes the birth ordinal
// instead. So a scene rendered with `gpu = true` is the same effect and not the
// same pixels as the same scene with it off. `birthsBy` is the part that must
// agree, and it is the part that is tested.

#include "generated/fields.slang"

/// One pooled particle. 32 bytes, mirroring `loom_render::gpu_particles::Slot`.
///
/// `float4` twice rather than two `float3`s for the reason `RainDrop` gives: a
/// `float3` in a buffer still aligns to 16, so the packed spelling costs
/// nothing and the two trailing scalars are free.
struct GpuParticle {
    /// xyz world position, w seconds lived.
    float4 position;
    /// xyz velocity in m/s, w the birth ordinal this slot currently holds, as
    /// a bit-cast `uint`. `GPU_PARTICLE_NONE` when the slot is empty.
    ///
    /// **The ordinal is the whole of the slot's identity.** A tick where the
    /// arithmetic hands this slot a different ordinal is that slot's birth
    /// instant; a tick where it hands back the same one is an ordinary step.
    /// There is no separate "alive" flag to get out of step with it.
    float4 velocity;
};

/// What the draw reads. Mirrors `ParticleInstance` in `scene.slang` exactly,
/// because the pool is drawn by **that** vertex shader and that pipeline —
/// there is no second particle renderer and there must not be one.
struct GpuParticleInstance {
    /// xyz world, w radius. Negative radius marks an additive particle.
    float4 position;
    /// rgb linear, a opacity. A negative red marks the flame field.
    float4 color;
};

/// A slot holding nothing. `0xFFFFFFFF` is unreachable as an ordinal: the pool
/// is refused above 65,536 slots and a `u32` of births would have to wrap.
static const uint GPU_PARTICLE_NONE = 0xFFFFFFFFu;

/// Everything about one emitter, by device address.
///
/// **Not in the push block**, which would need about 150 bytes for this and is
/// guaranteed only 128. Same answer the environment buffer gives the vertex
/// shaders: per-frame data that several stages read lives in a buffer.
struct GpuEmitterParams {
    /// xyz the emitter's world origin, w the spawn disc radius in metres.
    float4 origin;
    /// x initial speed, y cone half-angle in RADIANS, z gravity, w drag.
    float4 motion;
    /// x turbulence, y turbulence scale, z wind response, w lifetime.
    float4 swirl;
    /// x lifetime jitter, y rate per second, z delay, w duration.
    float4 life;
    /// x diameter at birth, y at death, z opacity at birth, w at death.
    float4 sizeAlpha;
    /// rgb linear colour at birth, w unused.
    float4 colorStart;
    /// rgb linear colour at death, w unused.
    float4 colorEnd;
    /// The wind field: x direction x, y direction z, z speed, w gustiness.
    ///
    /// **Unsheltered**, unlike the CPU emitter's, which samples
    /// `loom_field::wind::Wind` and applies S3's voxel occlusion. The shelter
    /// query is a CPU march of the SDF and there is no GPU form of it; grass
    /// takes the same unsheltered field from the environment buffer for the
    /// same reason.
    float4 wind;
    /// x turbulence, y ground drag, zw unused. The other half of `wind`, split
    /// the same way `EnvironmentData` splits it, so the two are filled from the
    /// same four numbers.
    float4 weather;
};

struct GpuParticlePush {
    /// x the fixed timestep in seconds, y unused, zw unused.
    ///
    /// **`dt` is passed rather than declared here**, so it is bit-for-bit the
    /// same `f32` the CPU twin divided by. `1.0 / 60.0` folded by two different
    /// compilers is a one-ulp risk on the boundary that decides how many
    /// particles exist.
    float4 clock;
    /// x pool slots, y ticks to advance, z 1 to clear the pool first, w the
    /// tick this buffer currently stands for.
    uint4 control;
    /// x the emitter's authored seed, y its burst count, z 1 for additive,
    /// w 1 for the flame field.
    uint4 ident;
    GpuEmitterParams* params;
    GpuParticle* pool;
    GpuParticleInstance* instances;
};

[[vk::push_constant]] GpuParticlePush gpuPush;

/// Four uniforms in `[0, 1)` from one seed. Same construction `rainHash4` uses,
/// off the same frozen `loom_hash` (ADR 0006), so this file adds no second
/// family of magic constants.
float4 gpuHash4(uint seed) {
    uint h1 = loom_hash(seed);
    uint h2 = loom_hash(h1);
    uint h3 = loom_hash(h2);
    uint h4 = loom_hash(h3);
    const float SCALE = 1.0 / 16777216.0;
    return float4(float(h1 >> 8), float(h2 >> 8), float(h3 >> 8), float(h4 >> 8)) * SCALE;
}

/// Particles released by the end of tick `tick`.
///
/// **One expression written twice** — the other copy is
/// `loom_particles::births_by` and a test compares them tick by tick. Every
/// line here has a counterpart there; changing one without the other changes
/// how many particles exist on the GPU alone, which is exactly the divergence
/// ADR 0006 exists to make impossible for fields and this comment exists to
/// make visible for populations.
uint gpuBirthsBy(uint tick) {
    GpuEmitterParams p = gpuPush.params[0];
    float dt = gpuPush.clock.x;
    if (dt <= 0.0) {
        return 0u;
    }
    float delay = max(p.life.z, 0.0);
    float first = ceil(delay / dt);
    float ticks = float(tick);
    if (ticks <= first) {
        return 0u;
    }
    float last = p.life.w > 0.0
        ? min(ticks, ceil((delay + p.life.w) / dt))
        : ticks;
    float emitting = max(last - first, 0.0);
    float continuous = floor(max(p.life.y, 0.0) * (emitting * dt));
    return gpuPush.ident.y + uint(continuous);
}

/// The birth ordinal slot `slot` holds once `births` particles have been
/// released, or [`GPU_PARTICLE_NONE`].
///
/// The largest `n < births` with `n ≡ slot (mod N)`. Integer arithmetic
/// throughout, so it cannot drift.
uint gpuOwnerOrdinal(uint slot, uint births, uint pool) {
    if (slot >= births) {
        return GPU_PARTICLE_NONE;
    }
    return ((births - 1u - slot) / pool) * pool + slot;
}

/// Seconds this ordinal gets. A pure function of the ordinal, so it does not
/// have to be stored and cannot get out of step with the slot.
float gpuLifetime(uint ordinal) {
    GpuEmitterParams p = gpuPush.params[0];
    float u = gpuHash4(loom_hash(ordinal ^ 0x5bf03635u)).x;
    return max(p.swirl.w * (1.0 + p.life.x * (u * 2.0 - 1.0)), 0.01);
}

/// A newly born particle, from its ordinal alone.
///
/// The spawn geometry is `loom_particles::System::spawn` transcribed: a disc
/// sampled with a square root so particles spread evenly over the *area*
/// rather than bunching into a stalk, and a direction inside a cone about +Y
/// with the same square-root weighting.
GpuParticle gpuSeed(uint ordinal) {
    GpuEmitterParams p = gpuPush.params[0];
    float4 a = gpuHash4((ordinal * 2654435761u) ^ gpuPush.ident.x);
    float4 b = gpuHash4(loom_hash((ordinal + 0x9e3779b9u) ^ gpuPush.ident.x));

    float angle = a.x * 6.2831853;
    float r = p.origin.w * sqrt(a.y);
    float3 position = p.origin.xyz + float3(cos(angle) * r, 0.0, sin(angle) * r);

    float theta = a.z * 6.2831853;
    float phi = p.motion.y * sqrt(a.w);
    float sp = sin(phi);
    float cp = cos(phi);
    float speed = p.motion.x * (0.7 + 0.6 * b.x);

    GpuParticle out;
    out.position = float4(position, 0.0);
    out.velocity = float4(sp * cos(theta) * speed, cp * speed, sp * sin(theta) * speed,
                          asfloat(ordinal));
    return out;
}

/// The swirl, sampled by *position* so two particles in the same place are
/// pushed the same way and the plume moves as a body.
///
/// `loom_value_noise` rather than the CPU's `loom_terrain::noise::value`: the
/// two are different noises and only the first has a Slang half. See the
/// header — this path is the same model, not the same pixels.
float3 gpuSwirl(float3 p, float scale, uint seed) {
    float3 q = p * scale;
    float3 offset = float3(
        float(loom_hash(seed ^ 0x11u) >> 8) * (1.0 / 16777216.0) * 64.0,
        float(loom_hash(seed ^ 0x22u) >> 8) * (1.0 / 16777216.0) * 64.0,
        float(loom_hash(seed ^ 0x33u) >> 8) * (1.0 / 16777216.0) * 64.0);
    return float3(
        loom_value_noise(float3(q.y, q.z, offset.x)) * 2.0 - 1.0,
        (loom_value_noise(float3(q.z, q.x, offset.y)) * 2.0 - 1.0) * 0.4,
        loom_value_noise(float3(q.x, q.y, offset.z)) * 2.0 - 1.0);
}

[shader("compute")]
[numthreads(64, 1, 1)]
void gpuParticleSimulateMain(uint3 tid: SV_DispatchThreadID) {
    uint slot = tid.x;
    uint pool = gpuPush.control.x;
    if (slot >= pool) {
        return;
    }

    GpuEmitterParams p = gpuPush.params[0];
    float dt = gpuPush.clock.x;
    uint steps = gpuPush.control.y;
    uint startTick = gpuPush.control.w;

    GpuParticle particle = gpuPush.pool[slot];
    if (gpuPush.control.z != 0u) {
        // An empty slot rather than a seeded one: which ordinals exist is the
        // arithmetic's answer, and pre-filling would be a second opinion.
        particle.position = float4(0.0, 0.0, 0.0, 0.0);
        particle.velocity = float4(0.0, 0.0, 0.0, asfloat(GPU_PARTICLE_NONE));
    }

    float damping = clamp(1.0 - p.motion.w * dt, 0.0, 1.0);
    // Filled field by field rather than through the generated `wind_at_params`
    // reader, exactly as `grassWind` does: the reader takes a `float*` and the
    // parameters here live inside a struct, so a pointer into it would be an
    // aliasing assumption for no gain.
    WindParams wind;
    wind.dir_x = p.wind.x;
    wind.dir_z = p.wind.y;
    wind.speed = p.wind.z;
    wind.gustiness = p.wind.w;
    wind.turbulence = p.weather.x;
    wind.ground_drag = p.weather.y;

    for (uint step = 0u; step < steps; ++step) {
        uint tick = startTick + step + 1u;
        uint ordinal = gpuOwnerOrdinal(slot, gpuBirthsBy(tick), pool);

        if (ordinal == GPU_PARTICLE_NONE) {
            // Nothing has been released into this slot yet. `births` never
            // decreases, so a slot that has an occupant never comes back here.
            continue;
        }
        if (ordinal != asuint(particle.velocity.w)) {
            // The slot changed hands: this tick is that ordinal's birth. A
            // newborn is *not* advanced on the tick it appears, matching
            // `System::step_in_wind`, which spawns after it advances.
            particle = gpuSeed(ordinal);
            continue;
        }

        float t = float(tick) * dt;
        particle.velocity.xyz += gpuSwirl(particle.position.xyz, p.swirl.y, gpuPush.ident.x)
            * (p.swirl.x * dt);
        particle.velocity.y += p.motion.z * dt;

        // **Pulled toward the air, not pushed by it**, so smoke ends up
        // travelling *with* the wind rather than accelerating downwind forever.
        // Clamped for the same reason drag is: a response above `1/dt` would
        // overshoot the air and oscillate.
        if (p.swirl.z > 0.0) {
            float3 air = wind_at(particle.position.xyz, t, wind);
            float follow = clamp(p.swirl.z * dt, 0.0, 1.0);
            particle.velocity.xyz += (air - particle.velocity.xyz) * follow;
        }

        particle.velocity.xyz *= damping;
        particle.position.xyz += particle.velocity.xyz * dt;
        particle.position.w += dt;
    }

    gpuPush.pool[slot] = particle;

    // The draw's instance, written by the same dispatch that produced the
    // state. A second pass would need a second barrier and would read exactly
    // these registers back out of memory.
    uint ordinal = asuint(particle.velocity.w);
    float life = ordinal == GPU_PARTICLE_NONE ? 1.0 : gpuLifetime(ordinal);
    float f = clamp(particle.position.w / life, 0.0, 1.0);

    GpuParticleInstance instance;
    if (ordinal == GPU_PARTICLE_NONE || particle.position.w >= life) {
        // **A degenerate quad, not a branch in the draw.** Six vertices at one
        // point rasterise nothing, which is cheaper than any mechanism for
        // telling the vertex shader that a slot is empty — and the vertex
        // shader is `scene.slang`'s, shared with the CPU path, which must not
        // grow a case for this.
        instance.position = float4(0.0, 0.0, 0.0, 0.0);
        instance.color = float4(0.0, 0.0, 0.0, 0.0);
    } else {
        float size = lerp(p.sizeAlpha.x, p.sizeAlpha.y, f);
        float radius = gpuPush.ident.z != 0u ? -size * 0.5 : size * 0.5;
        // Fade in as well as out: a particle that appears at full opacity pops,
        // and the pop is at the emitter where the eye already is. The flame
        // field opts out, exactly as the CPU path does, because a flame is one
        // long-lived quad whose `f` never leaves zero.
        float fade = gpuPush.ident.w != 0u ? 1.0 : min(f * 8.0, 1.0);
        float3 rgb = lerp(p.colorStart.rgb, p.colorEnd.rgb, f);
        float red = max(rgb.r, 1e-3) * (gpuPush.ident.w != 0u ? -1.0 : 1.0);
        instance.position = float4(particle.position.xyz, radius);
        instance.color = float4(red, rgb.g, rgb.b, lerp(p.sizeAlpha.z, p.sizeAlpha.w, f) * fade);
    }
    gpuPush.instances[slot] = instance;
}
