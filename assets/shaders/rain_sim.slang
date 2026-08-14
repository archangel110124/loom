// The raindrop simulation: integrate, collide against the world, recycle.
//
// **ADR 0014's deferral, fired.** The estimate that ADR wrote down was "a drop
// buffer, one compute dispatch per frame integrating and colliding against the
// voxel SDF, a splash append buffer feeding an indirect draw, and no readback
// ever". That is exactly this file.
//
// # Why the world reaches this shader as a 3D texture
//
// `loom_voxel::Volume` is a `BTreeMap` of chunks with an edit layer on top,
// which is a pointer chase no compute shader can walk. So the field is baked on
// the CPU by `loom_rain::collide` into an `R8_SNORM` 3D image and sampled here
// with a hardware trilinear fetch. The same argument
// `loom_voxel::heightfield` already makes for terrain height, and the same
// discipline: one bake, one formula, no second implementation.
//
// The bake is the **collision world** — voxels unioned with static box
// colliders — not the voxels alone. That is what fires ADR 0014's trigger 2: a
// mesh roof stops rain, which the baked height field could never express.
//
// # No cross-thread dependency, and what that buys
//
// Thread `i` reads and writes `drops[i]` and touches no other drop. So the
// whole simulation is 131,072 independent scalar recurrences, and advancing N
// ticks does **not** need N dispatches with N barriers between them: it needs
// one dispatch with the recurrence rolled up in registers. That is what
// `dims.w` is, and it is why catching a headless render up to `--sim 1800`
// costs one dispatch rather than 1,800.
//
// The one shared write is the splash ring's cursor, and it is an atomic on a
// buffer nothing in this dispatch reads.
//
// # Determinism
//
// Nothing here is read back, ever (`loom-water-system.md` 5.1, and Phase 4's
// structural rule). What it costs instead is that a frame stops being a pure
// function of the scene and the tick — it is now a function of the *sequence*
// of ticks the buffer has been advanced through. Seeding is therefore a pure
// function of the drop index and the eye, and a headless render seeds and then
// advances exactly `--sim` ticks in one dispatch, which is reproducible run to
// run. See ADR 0015 for what that guarantees and what it does not.

// `water.slang` for `LoomHeightField` and `loom_ground_height` — the terrain
// height grid, which is what a drop falls onto outside the baked collision
// field. Same order as `scene.slang`, and the unused half is stripped.
#include "generated/water.slang"
#include "generated/fields.slang"
#include "include/rain.slang"

struct RainSimPush {
    /// xyz eye position, w the simulated time at the END of this run.
    float4 eye;
    /// xyz wind in m/s, w the scene's unsheltered rate in mm/h.
    float4 wind;
    /// xyz world position of the collision field's voxel (0,0,0) **centre**,
    /// w metres per voxel.
    float4 field;
    /// xy world origin of the terrain height grid, z spacing, w samples per
    /// axis. Zero samples means the scene has no height grid.
    float4 terrain;
    /// xyz collision-field voxels per axis, w ticks to advance this dispatch.
    uint4 dims;
    /// x: 1 to seed the buffer before advancing. y: how many of the trailing
    /// ticks may append a splash.
    uint2 control;
    RainDrop* drops;
    RainSplash* splashes;
    /// The splash draw's `VkDrawIndirectCommand` in `[0..3]`, and the ring's
    /// write cursor in `[4]`.
    ///
    /// **One buffer for both** because the push block is at 120 of the 128
    /// bytes Vulkan guarantees, and a cursor that lives beside the arguments it
    /// determines is not obviously worse than one that lives apart from them.
    uint* splashArgs;
    /// The terrain heights, `terrain.w` squared of them, row-major.
    float* terrainHeights;
};

[[vk::push_constant]] RainSimPush rainPush;

/// The collision world. Set 0, binding 0.
///
/// `R8_SNORM`, so a fetch is in `[-1, 1]` and scales by `rainRange()` to
/// metres. Negative is inside a solid. Linear filtering, because a drop
/// crossing a voxel boundary must not step from air to rock — the same
/// argument `loom_voxel::exposure::sample` makes for interpolating rather than
/// taking the nearest voxel.
[[vk::binding(0, 0)]]
uniform Sampler3D rainField;

/// Metres the `i8` range spans either side of a surface.
///
/// **Two voxels, and it must stay in step with `loom_rain::collide::Field::range`.**
/// One memory layout described twice again; a mismatch here does not corrupt
/// anything, it moves the collision surface, which reads as rain landing inside
/// the floor.
float rainRange() {
    return rainPush.field.w * 2.0;
}

/// Normalised texture coordinates for a world point.
float3 rainFieldUv(float3 p) {
    float3 voxel = (p - rainPush.field.xyz) / rainPush.field.w;
    return (voxel + 0.5) / float3(rainPush.dims.xyz);
}

/// Signed distance in metres to the nearest solid, positive in air.
///
/// Outside the baked field this falls back to the terrain height grid, which is
/// the same `loom_ground_height` the streaks used to cull themselves against.
/// **The fallback is a height, not a distance**, so it cannot express an
/// overhang — but outside the collision field there is no overhang to express,
/// and the alternative is rain falling through the ground of any scene bigger
/// than the field.
float rainSolidAt(float3 p) {
    float3 uv = rainFieldUv(p);
    bool inside = all(uv > float3(0.0)) && all(uv < float3(1.0)) && rainPush.dims.x > 0;
    if (inside) {
        return rainField.SampleLevel(uv, 0.0).r * rainRange();
    }
    LoomHeightField grid;
    grid.origin = rainPush.terrain.xy;
    grid.spacing = rainPush.terrain.z;
    grid.side = int(rainPush.terrain.w);
    grid.height = rainPush.terrainHeights;
    return p.y - loom_ground_height(grid, p.xz);
}

/// The surface normal at a point inside a solid, by central differences.
///
/// Only evaluated when a drop has actually landed, which is a few hundred
/// threads a tick out of 131,072 — so six extra fetches on that path costs
/// nothing measurable and buys a splash that leans the way the roof does.
float3 rainSurfaceNormal(float3 p) {
    float h = rainPush.field.w;
    float3 n = float3(
        rainSolidAt(p + float3(h, 0, 0)) - rainSolidAt(p - float3(h, 0, 0)),
        rainSolidAt(p + float3(0, h, 0)) - rainSolidAt(p - float3(0, h, 0)),
        rainSolidAt(p + float3(0, 0, h)) - rainSolidAt(p - float3(0, 0, h)));
    float len = length(n);
    return len > 1e-5 ? n / len : float3(0, 1, 0);
}

/// Seed a drop anywhere in the block.
///
/// **Anywhere, so a fresh buffer is a volume of rain** rather than a sheet
/// above the camera that takes four seconds to arrive, and fully faded in — the
/// spawn fade exists to hide a drop reappearing from somewhere arbitrary, and
/// seeding is not that. Starting a seeded drop at age zero makes
/// `loom render --sim 0` a scene with no visible rain, which is exactly what it
/// did on this shader's first run.
RainDrop rainSeed(uint index, float3 eye, float3 wind) {
    float4 h = rainHash4(loom_hash(index * 2654435761u));
    RainDrop drop;
    drop.position = float4(rainBlockLow(eye) + h.xyz * RAIN_BOX, RAIN_SPAWN_FADE);
    uint seed = loom_hash(index);
    drop.velocity = float4(wind.x, -rainTerminal(seed), wind.z, asfloat(seed));
    return drop;
}

/// Metres a reissued drop is scattered sideways from where it landed.
///
/// Without any, a drop in a windless scene would fall down one line forever and
/// the field would slowly resolve into a set of fixed vertical tracks. With too
/// much it stops being a reissue *in the same column*, which is what the density
/// argument below rests on.
static const float RAIN_REISSUE_JITTER = 0.8;

/// Reissue a landed drop **one block-height above where it landed**.
///
/// **This is a density argument, and both of the obvious rules are wrong.**
///
/// *Reissue anywhere in the block* gives every drop the same chance of landing
/// over the ground as over anywhere else, while the drops over the ground are
/// *consumed* faster, because they have less room to fall. The equilibrium is a
/// deficit over the ground in the ratio of the fall heights — rain that thins
/// out as a scene runs.
///
/// *Reissue at the top of the block, in the same column* fixes the flow and
/// breaks the density the other way: a column whose ground is high then holds
/// the same number of drops over a shorter fall, so rain is densest exactly
/// where the ground is highest.
///
/// What has to be constant is the **distance a drop travels per cycle**, not
/// where it restarts. `hit.y + RAIN_BOX.y` makes every cycle exactly one block
/// height however high the ground is, so the visible density is the seeded
/// density everywhere and stays there. A drop spends the part of its cycle the
/// ground took away sitting above the block, where `rainEdgeFade` is zero and it
/// draws nothing — which is precisely the fraction of the column the ground
/// occupies. Measured on `rain_impact`: the sky's rain light is 1.27/255 at tick
/// 20 and 1.37 at tick 1,800, against 1.24 for the closed-form layer this
/// replaced.
///
/// The seed is rehashed with it, and that part is not cosmetic either: a drop
/// the rate has thinned *out* would keep that verdict for its whole life, so a
/// fixed seed would make light rain a set of permanently dry columns rather than
/// a sparser field.
RainDrop rainReissue(float3 hit, uint index, uint salt, float3 wind) {
    float4 h = rainHash4(loom_hash(index * 2654435761u + salt));
    RainDrop drop;
    drop.position = float4(
        hit.x + (h.x - 0.5) * RAIN_REISSUE_JITTER,
        hit.y + RAIN_BOX.y,
        hit.z + (h.z - 0.5) * RAIN_REISSUE_JITTER,
        0.0);
    uint seed = loom_hash(index ^ (salt * 0x9E3779B9u));
    drop.velocity = float4(wind.x, -rainTerminal(seed), wind.z, asfloat(seed));
    return drop;
}

/// Bring a position back into the neighbourhood of the block around the eye.
///
/// **The camera-motion seam, and wrapping is what makes it free.** As the eye
/// advances, drops fall out of the trailing face; wrapping puts each one at the
/// same offset into the leading face, so density is conserved exactly and no
/// drop is invented or destroyed. Both ends of that wrap sit in the outermost
/// eighth, where `rainEdgeFade` is zero.
///
/// **Vertically the range is two block heights, not one**, and that is
/// `rainReissue`'s headroom: a drop that landed on a roof is placed above the
/// block on purpose and has to be allowed to stay there until it has fallen back
/// in. Folding it into one height would drop it straight back onto the roof.
float3 rainWrap(float3 p, float3 eye) {
    float3 low = rainBlockLow(eye);
    // Horizontally, a plain modulo. `floor`-based, because it has to be correct
    // for a negative offset — which is where the closed-form layer's `%` bug
    // lived, and it cost most of the rain on one axis.
    float2 offset = p.xz - low.xz;
    float2 xz = low.xz + (offset - floor(offset / RAIN_BOX.xz) * RAIN_BOX.xz);

    // **Vertically it is a lift of exactly one block height, not a modulo into
    // the block, and the difference is the same density argument.** A drop that
    // falls out of the bottom having hit nothing must travel the same distance
    // per cycle as one that landed and was reissued — one `RAIN_BOX.y` — or a
    // column with no ground under it settles at half the density of one with a
    // floor. Folding into `[low, low + BOX.y)` cannot do that, because a
    // reissued drop needs headroom above the block and one range has to hold
    // both cases.
    float y = p.y;
    y += ceil(max(low.y - y, 0.0) / RAIN_BOX.y) * RAIN_BOX.y;
    // And the same in reverse, so a camera that dived does not leave drops
    // stranded far overhead.
    float ceiling = low.y + RAIN_BOX.y * 2.0;
    y -= ceil(max(y - ceiling, 0.0) / RAIN_BOX.y) * RAIN_BOX.y;
    return float3(xz.x, y, xz.y);
}

[shader("compute")]
[numthreads(64, 1, 1)]
void rainSimulateMain(uint3 tid: SV_DispatchThreadID) {
    uint index = tid.x;
    if (index >= RAIN_DROPS) {
        return;
    }

    float3 eye = rainPush.eye.xyz;
    float endTime = rainPush.eye.w;
    float3 wind = rainPush.wind.xyz;
    uint steps = rainPush.dims.w;

    RainDrop drop = rainPush.drops[index];
    if (rainPush.control.x != 0u) {
        drop = rainSeed(index, eye, wind);
    }

    // How many issues this drop has had. Rolled into the reissue hash so a drop
    // that lands twice does not land in the same place twice.
    uint issue = 0u;

    for (uint step = 0u; step < steps; ++step) {
        // The clock at the END of this step. A splash records when it happened,
        // and during a catch-up run those instants are spread across the run
        // rather than all landing on the final tick.
        float t = endTime - float(steps - 1u - step) * RAIN_TICK;
        uint seed = asuint(drop.velocity.w);

        // Relax toward the air's velocity. The drop keeps its own terminal
        // speed, so a gust turns the field without flattening it into one
        // object moving one way.
        float3 air = float3(wind.x, -rainTerminal(seed), wind.z);
        drop.velocity.xyz = lerp(drop.velocity.xyz, air, RAIN_DRAG);

        float3 from = drop.position.xyz;
        float3 to = from + drop.velocity.xyz * RAIN_TICK;
        float distance = rainSolidAt(to);

        if (distance <= 0.0) {
            // **The collision, and it is the whole reason this file exists.**
            // Project back out along the field's gradient: the SDF is accurate
            // near the surface, so `to + n * (-d)` lands on it.
            float3 normal = rainSurfaceNormal(to);
            float3 hit = to - normal * distance;

            if (step + rainPush.control.y >= steps && rainPresent(seed, rainPush.wind.w)) {
                // Append. The ring is sized so one tick's landings cannot lap
                // it, which is what keeps the drawn set deterministic even
                // though the order within it is not.
                uint slot;
                InterlockedAdd(rainPush.splashArgs[4], 1u, slot);
                RainSplash splash;
                splash.impact = float4(hit, t);
                splash.surface = float4(normal, length(drop.velocity.xyz));
                rainPush.splashes[slot % RAIN_SPLASH_RING] = splash;
            }

            issue += 1u;
            drop = rainReissue(hit, index, issue * 977u + step, wind);
            continue;
        }

        drop.position = float4(to, drop.position.w + RAIN_TICK);

        // Outside the block: wrap. Cheaper than a test per axis and exactly
        // conservative, which a respawn is not.
        float3 wrapped = rainWrap(drop.position.xyz, eye);
        if (any(abs(wrapped - drop.position.xyz) > float3(1e-3))) {
            drop.position = float4(wrapped, 0.0);
        }
    }

    rainPush.drops[index] = drop;
}

/// Write the splash draw's `VkDrawIndirectCommand` from the ring's cursor.
///
/// One thread. **A pass of its own** rather than a tail of the dispatch above,
/// because the cursor is being incremented by 131,072 threads in undefined
/// order there and reading it from inside the same dispatch would be a race
/// with no fixed answer.
[shader("compute")]
[numthreads(1, 1, 1)]
void rainSplashArgsMain() {
    uint written = min(rainPush.splashArgs[4], RAIN_SPLASH_RING);
    // vertexCount, instanceCount, firstVertex, firstInstance.
    rainPush.splashArgs[0] = written * 6u;
    rainPush.splashArgs[1] = 1u;
    rainPush.splashArgs[2] = 0u;
    rainPush.splashArgs[3] = 0u;
}
