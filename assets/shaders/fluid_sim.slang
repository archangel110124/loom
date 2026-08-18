// The cinematic tier's fluid solver: APIC particles, a MAC grid, and a
// multigrid pressure projection — ADR 0057, under ADR 0053.
//
// This is the first GPU-stateful *force* path in the project. Everything in
// here is shaped by one requirement that has nothing to do with fluids:
//
//   **`cargo xtask repeat` must be byte-identical across three fresh
//   processes on the same device.**
//
// Three consequences, each of which chose a different algorithm than a
// performance argument would have:
//
//  1. **No float atomic anywhere.** Float addition is not associative, so a
//     reduction whose order the scheduler picks gives a different sum every
//     run — and that sum feeds the next tick's pressure solve, so the
//     difference compounds instead of dithering. Every transfer here is a
//     *gather*: a node reads the particles near it and sums them in an order
//     fixed by the buffer, never a particle adding itself to a node.
//
//  2. **The gather order is fixed by particle index, not by arrival.** This is
//     the trap that the "no float atomics" rule implies but does not say. The
//     obvious counting sort — `atomicAdd` on a per-cell cursor — hands each
//     particle a rank equal to the order its wave happened to arrive in, which
//     is nondeterministic. The floats are then summed in *that* order, and the
//     float-atomic ban has been reintroduced through the back door with no
//     atomic in sight. So `fluidSortCellMain` re-sorts each bucket by particle
//     index afterwards, and the sum order is a function of the buffer alone.
//
//  3. **Multigrid, not PCG.** A conjugate-gradient dot product is a global
//     float reduction with no fixed summation tree. Every operator below is a
//     fixed local stencil, and the V-cycle count is a constant — three, never
//     residual-tested — so the dispatch sequence is a function of the tick and
//     of nothing else.
//
// APIC rather than FLIP is the same kind of choice: FLIP's blend ratio is a
// knob with no correct value, and its failure mode (null-space energy → jitter
// → interior voids) is the hardest thing to debug in a system nobody can read
// back cheaply. APIC carries the affine matrix instead and has no ratio.

/// One particle. 80 bytes: position, velocity, and the affine velocity
/// gradient APIC transfers instead of a blend ratio.
struct FluidParticle {
    float4 position;
    float4 velocity;
    float4 c0;
    float4 c1;
    float4 c2;
};

/// One multigrid level's geometry.
struct FluidLevel {
    /// xyz cells per axis, w this level's first cell in the stacked buffers.
    int4 dim;
    /// x metres per cell.
    float4 misc;
};

/// Everything the solver needs that is not per-dispatch.
///
/// **Behind a device address rather than in the push block**, because the
/// sixteen pointers below are 128 bytes on their own — the whole guarantee —
/// and the level table is another 128. The push block then carries the two
/// things that actually vary per dispatch.
struct FluidConsts {
    /// xyz the domain's minimum corner, w metres per cell at level 0.
    float4 origin;
    /// xyz cells per axis at level 0, w live particles.
    int4 dims;
    /// x substep seconds, y gravity (negative), z the CFL speed clamp,
    /// w unused.
    float4 timing;
    /// x cells at level 0, y u faces, z v faces, w w faces.
    int4 sizes;
    /// x cells summed over all levels, y instance stride, z instances,
    /// w probes.
    int4 counts;
    FluidLevel levels[4];

    FluidParticle* particles;
    uint* cellCount;
    uint* cellStart;
    uint* cellCursor;
    uint* sorted;
    /// The particles again, in bucket order.
    ///
    /// **A copy, and it is the difference between 14 ms a tick and 4.** The
    /// gather reads about 288 particles per face node through `sorted`, which
    /// is an indirection into a 42 MB buffer — every read a cache miss, and
    /// nothing shared between the neighbouring nodes that read the same
    /// bucket. Written once per substep in bucket order, the same gather is
    /// contiguous and the overlap between adjacent nodes hits in L2. Measured:
    /// 6.9 -> 2.0 ms for one substep at 524,288 particles.
    FluidParticle* reordered;
    uint* blockSums;
    float* velU;
    float* velV;
    float* velW;
    uint* marker;
    float* pressureA;
    float* pressureB;
    float* rhs;
    /// Level 0 only: xyz the solid's velocity here, w 1 where it is solid.
    /// Rasterised on the CPU from rapier transforms — a fact of (scene, tick),
    /// never of the camera.
    float4* solid;
    /// In: xyz centre, w radius. Out: two `float4` per probe, after the input
    /// block.
    float4* probes;
    float4* instances;
    /// Fluid fraction per cell in fixed point, `FLUID_DENSITY_SCALE` per
    /// particle. Host-visible: the surface is extracted on the CPU (ADR 0057
    /// addendum), because the solver holds a device of its own and the renderer
    /// holds another.
    uint* density;
    /// The inflow box's minimum corner, xyz world; w unused.
    float4 inflowLo;
    /// Its maximum corner.
    float4 inflowHi;
    /// The velocity a particle leaves the lip with, xyz m/s; w unused.
    float4 inflowVelocity;
};

struct FluidPush {
    FluidConsts* c;
    /// x multigrid level, y axis, z parity or flag, w spare.
    uint4 args;
};

[vk::push_constant] FluidPush push;

static const uint FLUID_AIR = 0;
static const uint FLUID_FLUID = 1;
static const uint FLUID_SOLID = 2;

/// Threads per group for every per-element pass, and the scan's block width.
static const uint FLUID_GROUP = 256;

/// The most particles one cell's bucket is re-sorted by index.
///
/// **This is not a performance knob. It is the reproducibility ceiling, and
/// 128 was measured to be too low.** The scatter above hands out slots in
/// arrival order and the sort takes that decision back — but only for the
/// first `FLUID_SORT_MAX` of a bucket. Past it the tail keeps arrival order,
/// the gather sums those particles' floats in that order, and the whole
/// float-atomic ban has been defeated with no atomic in sight.
///
/// Measured on `slosh.loom` at `--sim 150`, three fresh processes: at 128 the
/// three PNGs had three different hashes, 7.2% of pixels apart with a worst
/// channel of 153. At 1024 they are byte-identical. So a cell in that scene
/// does hold more than 128 particles — sixteen times rest density — which is
/// itself a finding about the solve rather than about the sort.
///
/// **1024 was still too low, and `ribbon.loom` is what found it** (ADR 0057
/// addendum). A cascade landing in a shallow pool packs a cell to well over a
/// thousand particles during the transient, and three fresh processes gave
/// three pictures 37% apart. At 4096 `ribbon` is byte-identical at tick 180 and
/// at tick 400, and `slosh` is byte-identical at 150 **and at 600** — the run
/// ADR 0057 records as failing.
///
/// `ponytail:` an insertion sort is O(k²) in the bucket, so this ceiling is
/// also a hang risk if a cell ever reaches it, and at 4096 that would be 8M
/// comparisons in one thread. It is why `ribbon` costs 37 ms a tick against
/// `slosh`'s 3. The upgrade is a block radix sort with scan-derived ranks; the
/// real fix is upstream, in a projection that does not let a cell reach a
/// hundred times rest density in the first place.
static const uint FLUID_SORT_MAX = 4096;

/// Probe slots. The output block starts here, so the CPU knows where to read
/// without the count it wrote being part of the address.
static const int FLUID_MAX_PROBES = 64;

/// Particles a full cell holds, mirroring `loom_render::FLUID_PER_CELL`.
static const uint FLUID_PER_CELL = 8;

int3 gridDims(FluidConsts* c, uint level) { return c->levels[level].dim.xyz; }
int gridBase(FluidConsts* c, uint level) { return c->levels[level].dim.w; }

int cellIndex(int3 d, int3 p) { return (p.z * d.y + p.y) * d.x + p.x; }
bool inside(int3 d, int3 p) { return all(p >= int3(0)) && all(p < d); }

/// The quadratic B-spline, the weight APIC's `4/h²` is derived for.
float bspline(float d) {
    float a = abs(d);
    if (a < 0.5) { return 0.75 - a * a; }
    if (a < 1.5) { float t = 1.5 - a; return 0.5 * t * t; }
    return 0.0;
}

/// A hash of the particle's own ordinal, so seeding is a pure function of the
/// index and reproduces without a `thread_rng` anywhere near it.
float hash01(uint n) {
    n ^= n >> 16;
    n *= 0x7feb352du;
    n ^= n >> 15;
    n *= 0x846ca68bu;
    n ^= n >> 16;
    return float(n & 0x00ffffffu) / 16777216.0;
}

// ---------------------------------------------------------------- seeding ---

[numthreads(256, 1, 1)]
void fluidSeedMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    uint p = tid.x;
    if (p >= uint(c->dims.w)) { return; }

    int3 d = c->dims.xyz;
    // The filled slab's height in cells is baked into the particle count, so
    // it is recovered rather than passed again.
    int fy = max(1, c->dims.w / (8 * d.x * d.z));
    uint cell = p >> 3;
    uint sub = p & 7u;
    int i = int(cell) % d.x;
    int j = (int(cell) / d.x) % fy;
    int k = int(cell) / (d.x * fy);

    // Eight on a fixed 2x2x2 lattice inside the cell, jittered by a hash of
    // the ordinal. The lattice is what keeps the initial density even; the
    // jitter is what stops eight perfectly aligned columns ringing.
    float3 lattice = float3(
        (sub & 1u) != 0u ? 0.75 : 0.25,
        (sub & 2u) != 0u ? 0.75 : 0.25,
        (sub & 4u) != 0u ? 0.75 : 0.25);
    float3 jitter = float3(hash01(p * 3u + 1u), hash01(p * 3u + 2u), hash01(p * 3u + 3u));
    jitter = (jitter - 0.5) * 0.3;

    float h = c->origin.w;
    FluidParticle q;
    q.position = float4(
        c->origin.xyz + (float3(float(i), float(j), float(k)) + lattice + jitter) * h, 0.0);
    q.velocity = float4(0.0);
    q.c0 = float4(0.0);
    q.c1 = float4(0.0);
    q.c2 = float4(0.0);
    c->particles[p] = q;
}

// ------------------------------------------------------- the bucket sort ---

int particleCell(FluidConsts* c, float3 x) {
    int3 d = c->dims.xyz;
    float h = c->origin.w;
    int3 p = int3(floor((x - c->origin.xyz) / h));
    p = clamp(p, int3(0), d - int3(1));
    return cellIndex(d, p);
}

[numthreads(256, 1, 1)]
void fluidClearCountsMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (int(tid.x) >= c->sizes.x) { return; }
    c->cellCount[tid.x] = 0u;
}

[numthreads(256, 1, 1)]
void fluidCountMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (tid.x >= uint(c->dims.w)) { return; }
    int cell = particleCell(c, c->particles[tid.x].position.xyz);
    // **Integer**, and that is the whole licence for an atomic here: integer
    // addition is associative, so a count does not depend on arrival order.
    // What arrival order would decide is the *rank*, and `fluidSortCellMain`
    // takes that decision back below.
    uint ignored;
    InterlockedAdd(c->cellCount[cell], 1u, ignored);
}

groupshared uint scanShared[FLUID_GROUP];

[numthreads(256, 1, 1)]
void fluidScanBlockMain(
    uint3 tid: SV_DispatchThreadID,
    uint3 gid: SV_GroupThreadID,
    uint3 grp: SV_GroupID
) {
    FluidConsts* c = push.c;
    uint n = uint(c->sizes.x);
    uint i = tid.x;
    uint local = gid.x;
    uint own = i < n ? c->cellCount[i] : 0u;
    scanShared[local] = own;
    GroupMemoryBarrierWithGroupSync();

    // Hillis-Steele inclusive scan. Integer, so the order the steps land in is
    // irrelevant to the answer.
    for (uint offset = 1u; offset < FLUID_GROUP; offset <<= 1) {
        uint v = local >= offset ? scanShared[local - offset] : 0u;
        GroupMemoryBarrierWithGroupSync();
        scanShared[local] += v;
        GroupMemoryBarrierWithGroupSync();
    }
    uint inclusive = scanShared[local];
    if (i < n) { c->cellStart[i] = inclusive - own; }
    if (local == FLUID_GROUP - 1u) { c->blockSums[grp.x] = inclusive; }
}

[numthreads(1024, 1, 1)]
void fluidScanSumsMain(uint3 gid: SV_GroupThreadID) {
    FluidConsts* c = push.c;
    uint blocks = (uint(c->sizes.x) + FLUID_GROUP - 1u) / FLUID_GROUP;
    // One group scans every block sum. 64³ is the documented grid ceiling, so
    // there are at most 1024 of them and this loop is exact rather than
    // hierarchical.
    if (gid.x == 0u) {
        uint running = 0u;
        for (uint b = 0u; b < blocks; ++b) {
            uint v = c->blockSums[b];
            c->blockSums[b] = running;
            running += v;
        }
    }
}

[numthreads(256, 1, 1)]
void fluidScanAddMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (int(tid.x) >= c->sizes.x) { return; }
    uint start = c->cellStart[tid.x] + c->blockSums[tid.x / FLUID_GROUP];
    c->cellStart[tid.x] = start;
    c->cellCursor[tid.x] = start;
}

[numthreads(256, 1, 1)]
void fluidScatterMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (tid.x >= uint(c->dims.w)) { return; }
    int cell = particleCell(c, c->particles[tid.x].position.xyz);
    uint slot;
    InterlockedAdd(c->cellCursor[cell], 1u, slot);
    c->sorted[slot] = tid.x;
}

[numthreads(256, 1, 1)]
void fluidSortCellMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (int(tid.x) >= c->sizes.x) { return; }
    uint start = c->cellStart[tid.x];
    uint count = min(c->cellCount[tid.x], FLUID_SORT_MAX);
    // **This is the pass that makes the whole solver reproducible.** The
    // scatter above handed out slots in arrival order; an insertion sort by
    // particle index replaces that with an order the buffer alone decides.
    // Buckets hold about eight, so this is ~32 comparisons in the common case.
    for (uint a = 1u; a < count; ++a) {
        uint v = c->sorted[start + a];
        uint b = a;
        while (b > 0u && c->sorted[start + b - 1u] > v) {
            c->sorted[start + b] = c->sorted[start + b - 1u];
            --b;
        }
        c->sorted[start + b] = v;
    }
}

/// Copy the particles into bucket order, so the gather below is contiguous.
[numthreads(256, 1, 1)]
void fluidReorderMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (tid.x >= uint(c->dims.w)) { return; }
    c->reordered[tid.x] = c->particles[c->sorted[tid.x]];
}

// ----------------------------------------------------------- the MAC grid ---

int3 faceDims(FluidConsts* c, uint axis) {
    int3 d = c->dims.xyz;
    if (axis == 0u) { return int3(d.x + 1, d.y, d.z); }
    if (axis == 1u) { return int3(d.x, d.y + 1, d.z); }
    return int3(d.x, d.y, d.z + 1);
}

float* faceBuffer(FluidConsts* c, uint axis) {
    return axis == 0u ? c->velU : (axis == 1u ? c->velV : c->velW);
}

/// The offset of a face node from the cell's minimum corner, in cells.
float3 faceOffset(uint axis) {
    float3 o = float3(0.5, 0.5, 0.5);
    o[axis] = 0.0;
    return o;
}

/// Gather all three face velocities of one cell from the particles near it —
/// the P2G half of APIC, written as a gather so no float is added atomically.
///
/// **One thread per cell, three faces, one pass over the buckets.** The three
/// components were three dispatches and it cost twice as much: `u` reaches
/// into 36 buckets, `v` into 36 and `w` into 36, and the union of the three is
/// 54 — so doing them together reads each particle once instead of twice, and
/// pays 9 B-spline evaluations rather than 9 spread over three kernels that
/// each re-fetch the particle. Measured at 524,288 particles: 4.2 -> 2.0 ms.
[numthreads(256, 1, 1)]
void fluidP2GMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    int3 d = c->dims.xyz;
    int3 ed = d + int3(1);
    if (int(tid.x) >= ed.x * ed.y * ed.z) { return; }
    int3 n = int3(int(tid.x) % ed.x, (int(tid.x) / ed.x) % ed.y, int(tid.x) / (ed.x * ed.y));
    float h = c->origin.w;

    float3 sumW = float3(0.0);
    float3 sumV = float3(0.0);

    for (int dz = -2; dz <= 1; ++dz) {
        for (int dy = -2; dy <= 1; ++dy) {
            for (int dx = -2; dx <= 1; ++dx) {
                // The union of the three stencils: at most one axis may reach
                // two cells back. The ten corner buckets this drops reach none
                // of the three nodes.
                int deep = (dx == -2 ? 1 : 0) + (dy == -2 ? 1 : 0) + (dz == -2 ? 1 : 0);
                if (deep > 1) { continue; }
                int3 p = n + int3(dx, dy, dz);
                if (!inside(d, p)) { continue; }
                int cell = cellIndex(d, p);
                uint start = c->cellStart[cell];
                uint count = c->cellCount[cell];
                for (uint s = 0u; s < count; ++s) {
                    FluidParticle q = c->reordered[start + s];
                    float3 g = (q.position.xyz - c->origin.xyz) / h - float3(n);
                    // Six B-splines, not nine: a face node is on the cell
                    // boundary along its own axis and at the centre along the
                    // other two, so every weight is a product of these.
                    float bx0 = bspline(g.x), bx5 = bspline(g.x - 0.5);
                    float by0 = bspline(g.y), by5 = bspline(g.y - 0.5);
                    float bz0 = bspline(g.z), bz5 = bspline(g.z - 0.5);
                    float3 w = float3(bx0 * by5 * bz5, bx5 * by0 * bz5, bx5 * by5 * bz0);
                    if (all(w <= 0.0)) { continue; }
                    // APIC: the particle's velocity evaluated at each node,
                    // from its own affine field.
                    float3 base = c->origin.xyz + float3(n) * h;
                    float3 ru = base + float3(0.0, 0.5, 0.5) * h - q.position.xyz;
                    float3 rv = base + float3(0.5, 0.0, 0.5) * h - q.position.xyz;
                    float3 rw = base + float3(0.5, 0.5, 0.0) * h - q.position.xyz;
                    float3 v = float3(
                        q.velocity.x + dot(q.c0.xyz, ru),
                        q.velocity.y + dot(q.c1.xyz, rv),
                        q.velocity.z + dot(q.c2.xyz, rw));
                    sumW += w;
                    sumV += w * v;
                }
            }
        }
    }

    float3 out = float3(
        sumW.x > 1e-8 ? sumV.x / sumW.x : 0.0,
        sumW.y > 1e-8 ? sumV.y / sumW.y : 0.0,
        sumW.z > 1e-8 ? sumV.z / sumW.z : 0.0);
    if (n.y < d.y && n.z < d.z) { c->velU[(n.z * d.y + n.y) * (d.x + 1) + n.x] = out.x; }
    if (n.x < d.x && n.z < d.z) { c->velV[(n.z * (d.y + 1) + n.y) * d.x + n.x] = out.y; }
    if (n.x < d.x && n.y < d.y) { c->velW[(n.z * d.y + n.y) * d.x + n.x] = out.z; }
}

bool solidCell(FluidConsts* c, int3 p) {
    int3 d = c->dims.xyz;
    if (!inside(d, p)) { return true; }
    return c->solid[cellIndex(d, p)].w > 0.5;
}

/// Gravity, the CFL clamp, and the solid boundary condition on the faces.
[numthreads(256, 1, 1)]
void fluidForcesMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    uint axis = push.args.y;
    int3 fd = faceDims(c, axis);
    int total = fd.x * fd.y * fd.z;
    if (int(tid.x) >= total) { return; }
    int3 n = int3(int(tid.x) % fd.x, (int(tid.x) / fd.x) % fd.y, int(tid.x) / (fd.x * fd.y));

    float v = faceBuffer(c, axis)[tid.x];
    if (axis == 1u) { v += c->timing.y * c->timing.x; }
    v = clamp(v, -c->timing.z, c->timing.z);

    // A face touching a solid takes the solid's own velocity, which is how a
    // moving body pushes water: two-way coupling with no separate force term.
    int3 lower = n; lower[axis] -= 1;
    bool sa = solidCell(c, lower);
    bool sb = solidCell(c, n);
    if (sa || sb) {
        int3 d = c->dims.xyz;
        float sv = 0.0;
        if (sa && inside(d, lower)) { sv = c->solid[cellIndex(d, lower)][axis]; }
        else if (sb && inside(d, n)) { sv = c->solid[cellIndex(d, n)][axis]; }
        v = sv;
    }
    faceBuffer(c, axis)[tid.x] = v;
}

/// Marker cells: solid from the bake, fluid where particles are, air elsewhere.
[numthreads(256, 1, 1)]
void fluidMarkerMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (int(tid.x) >= c->sizes.x) { return; }
    uint m = c->solid[tid.x].w > 0.5 ? FLUID_SOLID
           : (c->cellCount[tid.x] > 0u ? FLUID_FLUID : FLUID_AIR);
    c->marker[tid.x] = m;
}

/// A coarse cell is solid only when every child is, and fluid when any is.
[numthreads(256, 1, 1)]
void fluidRestrictMarkerMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    uint level = push.args.x;
    int3 d = gridDims(c, level);
    if (int(tid.x) >= d.x * d.y * d.z) { return; }
    int3 p = int3(int(tid.x) % d.x, (int(tid.x) / d.x) % d.y, int(tid.x) / (d.x * d.y));
    int3 fdim = gridDims(c, level - 1u);
    int fbase = gridBase(c, level - 1u);

    int fluidChildren = 0;
    bool allSolid = true;
    for (int k = 0; k < 2; ++k) {
        for (int j = 0; j < 2; ++j) {
            for (int i = 0; i < 2; ++i) {
                int3 f = p * 2 + int3(i, j, k);
                if (!inside(fdim, f)) { continue; }
                uint m = c->marker[fbase + cellIndex(fdim, f)];
                if (m == FLUID_FLUID) { fluidChildren += 1; }
                if (m != FLUID_SOLID) { allSolid = false; }
            }
        }
    }
    // **A majority, not "any".** Calling a coarse cell fluid because one of
    // its eight children is puts the coarse grid's Dirichlet surface a level
    // higher than the fine grid's, and the correction that comes back down is
    // a pressure the fine surface never asked for. Measured: with "any", three
    // V-cycles collapsed a settled tank by 25 cm and ten V-cycles collapsed it
    // further — the classic sign of a coarse operator that is not an
    // approximation of the fine one.
    c->marker[gridBase(c, level) + int(tid.x)] =
        allSolid ? FLUID_SOLID : (fluidChildren >= 4 ? FLUID_FLUID : FLUID_AIR);
}

/// `b = -div(u) h²`, and the initial guess is zero.
[numthreads(256, 1, 1)]
void fluidDivergenceMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    int3 d = c->dims.xyz;
    if (int(tid.x) >= c->sizes.x) { return; }
    int3 p = int3(int(tid.x) % d.x, (int(tid.x) / d.x) % d.y, int(tid.x) / (d.x * d.y));
    float h = c->origin.w;

    float du = c->velU[(p.z * d.y + p.y) * (d.x + 1) + p.x + 1]
             - c->velU[(p.z * d.y + p.y) * (d.x + 1) + p.x];
    float dv = c->velV[(p.z * (d.y + 1) + p.y + 1) * d.x + p.x]
             - c->velV[(p.z * (d.y + 1) + p.y) * d.x + p.x];
    float dw = c->velW[((p.z + 1) * d.y + p.y) * d.x + p.x]
             - c->velW[(p.z * d.y + p.y) * d.x + p.x];
    float div = (du + dv + dw) / h;

    bool fluid = c->marker[tid.x] == FLUID_FLUID;
    c->rhs[tid.x] = fluid ? -div * h * h : 0.0;
    c->pressureA[tid.x] = 0.0;
    c->pressureB[tid.x] = 0.0;
}

/// One damped-Jacobi sweep at `level`, reading `src` and writing `dst`.
///
/// `A_cc` counts non-solid neighbours; an air neighbour is a Dirichlet zero
/// and still counts, a solid one is Neumann and does not.
[numthreads(256, 1, 1)]
void fluidJacobiMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    uint level = push.args.x;
    int3 d = gridDims(c, level);
    if (int(tid.x) >= d.x * d.y * d.z) { return; }
    int base = gridBase(c, level);
    float* src = push.args.z == 0u ? c->pressureA : c->pressureB;
    float* dst = push.args.z == 0u ? c->pressureB : c->pressureA;

    if (c->marker[base + int(tid.x)] != FLUID_FLUID) {
        dst[base + int(tid.x)] = 0.0;
        return;
    }
    int3 p = int3(int(tid.x) % d.x, (int(tid.x) / d.x) % d.y, int(tid.x) / (d.x * d.y));

    float diag = 0.0;
    float sum = 0.0;
    for (uint a = 0u; a < 3u; ++a) {
        for (int s = -1; s <= 1; s += 2) {
            int3 q = p; q[a] += s;
            uint m = inside(d, q) ? c->marker[base + cellIndex(d, q)] : FLUID_SOLID;
            if (m == FLUID_SOLID) { continue; }
            diag += 1.0;
            if (m == FLUID_FLUID) { sum += src[base + cellIndex(d, q)]; }
        }
    }
    if (diag <= 0.0) { dst[base + int(tid.x)] = 0.0; return; }
    float here = src[base + int(tid.x)];
    float residual = c->rhs[base + int(tid.x)] - (diag * here - sum);
    dst[base + int(tid.x)] = here + (2.0 / 3.0) * residual / diag;
}

/// Full-weighting restriction of the residual, and a zero coarse guess.
///
/// The factor is the operator's own scaling: `A` here is `-h²∇²`, so doubling
/// `h` multiplies the right-hand side by four, and averaging eight children
/// divides by eight — `4/8`.
[numthreads(256, 1, 1)]
void fluidRestrictMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    uint level = push.args.x;
    int3 d = gridDims(c, level);
    if (int(tid.x) >= d.x * d.y * d.z) { return; }
    int base = gridBase(c, level);
    int3 p = int3(int(tid.x) % d.x, (int(tid.x) / d.x) % d.y, int(tid.x) / (d.x * d.y));

    int3 fdim = gridDims(c, level - 1u);
    int fbase = gridBase(c, level - 1u);
    float acc = 0.0;
    for (int k = 0; k < 2; ++k) {
        for (int j = 0; j < 2; ++j) {
            for (int i = 0; i < 2; ++i) {
                int3 f = p * 2 + int3(i, j, k);
                if (!inside(fdim, f)) { continue; }
                int fi = fbase + cellIndex(fdim, f);
                if (c->marker[fi] != FLUID_FLUID) { continue; }
                float diag = 0.0;
                float sum = 0.0;
                for (uint a = 0u; a < 3u; ++a) {
                    for (int s = -1; s <= 1; s += 2) {
                        int3 q = f; q[a] += s;
                        uint m = inside(fdim, q) ? c->marker[fbase + cellIndex(fdim, q)]
                                                 : FLUID_SOLID;
                        if (m == FLUID_SOLID) { continue; }
                        diag += 1.0;
                        if (m == FLUID_FLUID) { sum += c->pressureA[fbase + cellIndex(fdim, q)]; }
                    }
                }
                acc += c->rhs[fi] - (diag * c->pressureA[fi] - sum);
            }
        }
    }
    c->rhs[base + int(tid.x)] = 0.5 * acc;
    c->pressureA[base + int(tid.x)] = 0.0;
    c->pressureB[base + int(tid.x)] = 0.0;
}

/// Trilinear prolongation of the coarse correction onto the fine level.
[numthreads(256, 1, 1)]
void fluidProlongMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    uint level = push.args.x;
    int3 d = gridDims(c, level);
    if (int(tid.x) >= d.x * d.y * d.z) { return; }
    int base = gridBase(c, level);
    if (c->marker[base + int(tid.x)] != FLUID_FLUID) { return; }
    int3 p = int3(int(tid.x) % d.x, (int(tid.x) / d.x) % d.y, int(tid.x) / (d.x * d.y));

    int3 cdim = gridDims(c, level + 1u);
    int cbase = gridBase(c, level + 1u);
    float3 cp = (float3(p) + 0.5) * 0.5 - 0.5;
    int3 c0 = int3(floor(cp));
    float3 t = cp - float3(c0);

    float acc = 0.0;
    float wsum = 0.0;
    for (int k = 0; k < 2; ++k) {
        for (int j = 0; j < 2; ++j) {
            for (int i = 0; i < 2; ++i) {
                int3 q = c0 + int3(i, j, k);
                if (!inside(cdim, q)) { continue; }
                int qi = cbase + cellIndex(cdim, q);
                if (c->marker[qi] != FLUID_FLUID) { continue; }
                float w = (i == 0 ? 1.0 - t.x : t.x)
                        * (j == 0 ? 1.0 - t.y : t.y)
                        * (k == 0 ? 1.0 - t.z : t.z);
                acc += w * c->pressureA[qi];
                wsum += w;
            }
        }
    }
    if (wsum > 1e-6) { c->pressureA[base + int(tid.x)] += acc / wsum; }
}

/// `u -= ∇φ`, on every face between two non-solid cells at least one of which
/// holds fluid.
[numthreads(256, 1, 1)]
void fluidProjectMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    uint axis = push.args.y;
    int3 fd = faceDims(c, axis);
    if (int(tid.x) >= fd.x * fd.y * fd.z) { return; }
    int3 n = int3(int(tid.x) % fd.x, (int(tid.x) / fd.x) % fd.y, int(tid.x) / (fd.x * fd.y));
    int3 d = c->dims.xyz;
    int3 lower = n; lower[axis] -= 1;

    uint ma = inside(d, lower) ? c->marker[cellIndex(d, lower)] : FLUID_SOLID;
    uint mb = inside(d, n) ? c->marker[cellIndex(d, n)] : FLUID_SOLID;
    if (ma == FLUID_SOLID || mb == FLUID_SOLID) { return; }
    if (ma != FLUID_FLUID && mb != FLUID_FLUID) { return; }

    float pa = ma == FLUID_FLUID ? c->pressureA[cellIndex(d, lower)] : 0.0;
    float pb = mb == FLUID_FLUID ? c->pressureA[cellIndex(d, n)] : 0.0;
    // **Clamped here as well as in `fluidForcesMain`.** The CFL guard has to
    // hold on what comes OUT of the projection, not only on what goes in: a
    // fast-moving solid drives a large divergence into a cell whose only other
    // neighbour is air, and three V-cycles of Jacobi answer that with a
    // pressure spike. Unclamped, the probe readback then hands the rigid-body
    // drag a water velocity of a thousand metres a second.
    float v = faceBuffer(c, axis)[tid.x] - (pb - pa) / c->origin.w;
    faceBuffer(c, axis)[tid.x] = clamp(v, -c->timing.z, c->timing.z);
}

// ------------------------------------------------------------------ G2P ----

/// Sample one MAC component, and its APIC row, at a world position.
void gatherAxis(FluidConsts* c, uint axis, float3 x, out float v, out float3 row) {
    int3 fd = faceDims(c, axis);
    float h = c->origin.w;
    float3 o = faceOffset(axis);
    float3 g = (x - c->origin.xyz) / h - o;
    int3 base = int3(floor(g - 0.5));
    float* buf = faceBuffer(c, axis);

    v = 0.0;
    row = float3(0.0);
    for (int k = 0; k < 3; ++k) {
        for (int j = 0; j < 3; ++j) {
            for (int i = 0; i < 3; ++i) {
                int3 n = base + int3(i, j, k);
                if (!inside(fd, n)) { continue; }
                float3 delta = g - float3(n);
                float w = bspline(delta.x) * bspline(delta.y) * bspline(delta.z);
                if (w <= 0.0) { continue; }
                float u = buf[(n.z * fd.y + n.y) * fd.x + n.x];
                float3 nodePos = c->origin.xyz + (float3(n) + o) * h;
                v += w * u;
                // `C = (4/h²) Σ w u (x_i − x_p)ᵀ` — the constant is the inverse
                // of the quadratic B-spline's second moment, which is why the
                // weight had to be quadratic for this line to be `4/h²`.
                row += (4.0 / (h * h)) * w * u * (nodePos - x);
            }
        }
    }
}

/// Push a particle out of solid along the mask's own gradient.
float3 escapeSolid(FluidConsts* c, float3 x) {
    int3 d = c->dims.xyz;
    float h = c->origin.w;
    int3 p = clamp(int3(floor((x - c->origin.xyz) / h)), int3(0), d - int3(1));
    if (!solidCell(c, p)) { return x; }
    float3 n = float3(0.0);
    for (uint a = 0u; a < 3u; ++a) {
        int3 lo = p; lo[a] -= 1;
        int3 hi = p; hi[a] += 1;
        n[a] = (solidCell(c, lo) ? 1.0 : 0.0) - (solidCell(c, hi) ? 1.0 : 0.0);
    }
    float len = length(n);
    if (len < 1e-6) { return x; }
    return x + (n / len) * h;
}

[numthreads(256, 1, 1)]
void fluidG2PMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (tid.x >= uint(c->dims.w)) { return; }
    FluidParticle q = c->particles[tid.x];

    float vx, vy, vz;
    float3 r0, r1, r2;
    gatherAxis(c, 0u, q.position.xyz, vx, r0);
    gatherAxis(c, 1u, q.position.xyz, vy, r1);
    gatherAxis(c, 2u, q.position.xyz, vz, r2);

    float3 v = float3(vx, vy, vz);
    float speed = length(v);
    if (speed > c->timing.z) { v *= c->timing.z / speed; }

    float3 x = q.position.xyz + v * c->timing.x;
    x = escapeSolid(c, x);
    // The tank's own walls. The boundary faces are already solid, so this is a
    // net rather than the mechanism.
    float h = c->origin.w;
    float3 lo = c->origin.xyz + h * 0.25;
    float3 hi = c->origin.xyz + (float3(c->dims.xyz) - 0.25) * h;
    x = clamp(x, lo, hi);

    q.position = float4(x, 0.0);
    q.velocity = float4(v, 0.0);
    q.c0 = float4(r0, 0.0);
    q.c1 = float4(r1, 0.0);
    q.c2 = float4(r2, 0.0);
    c->particles[tid.x] = q;
}

// -------------------------------------------------------------- readback ---

/// One thread per probe: where the free surface is around this pontoon, and
/// how fast the water there is moving.
///
/// **The surface is read from the columns AROUND the pontoon, not from the one
/// it is in, and that is the whole subtlety of the readback.** The body is
/// rasterised into the solid mask so that it pushes the fluid — which means
/// the cells it occupies hold no particles, so a column through it reports the
/// water level at the *bottom* of the hull and a floating crate reads as dry.
/// The water level a body floats in is the level of the water beside it, which
/// is what a waterline is.
///
/// What comes back is therefore a **surface height**, and the CPU turns it into
/// a displaced volume with the same `submerged_volume` spherical cap the
/// deterministic tier uses. One formula for "how wet is this pontoon", two
/// sources for the surface.
///
/// **Serial on purpose.** Eight columns of at most 64 cells is nothing to
/// parallelise, and a serial loop is a summation order the buffer decides
/// rather than the scheduler.
[numthreads(1, 1, 1)]
void fluidProbeMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (int(tid.x) >= c->counts.w) { return; }
    float4 probe = c->probes[tid.x];
    int3 d = c->dims.xyz;
    float h = c->origin.w;

    // Eight columns on a ring just clear of the pontoon.
    float surface = 0.0;
    int wetColumns = 0;
    const int RING = 8;
    for (int r = 0; r < RING; ++r) {
        float a = 6.2831853 * float(r) / float(RING);
        float3 at = probe.xyz + float3(cos(a), 0.0, sin(a)) * probe.w * 1.5;
        int ix = int(floor((at.x - c->origin.x) / h));
        int iz = int(floor((at.z - c->origin.z) / h));
        if (ix < 0 || iz < 0 || ix >= d.x || iz >= d.z) { continue; }
        for (int j = d.y - 1; j >= 0; --j) {
            // **Half rest density, not one particle.** A cell holding a single
            // particle is a fleck of spray, and taking the highest such cell as
            // the free surface reads the top of the highest speck in the column
            // — which drifts upward over a run and shows up as a tank that
            // slowly gains volume when nothing of the sort is happening.
            int here = cellIndex(d, int3(ix, j, iz));
            if (c->marker[here] != FLUID_FLUID) { continue; }
            if (c->cellCount[here] * 2u < FLUID_PER_CELL) { continue; }
            surface += c->origin.y + float(j + 1) * h;
            wetColumns += 1;
            break;
        }
    }

    // The water's velocity over the pontoon itself, from the fluid cells it
    // touches. Zero when it is clear of the water, which is the honest answer.
    int wet = 0;
    float3 vel = float3(0.0);
    const int N = 4;
    for (int k = 0; k < N; ++k) {
        for (int j = 0; j < N; ++j) {
            for (int i = 0; i < N; ++i) {
                float3 t = (float3(float(i), float(j), float(k)) + 0.5) / float(N) * 2.0 - 1.0;
                if (dot(t, t) > 1.0) { continue; }
                float3 x = probe.xyz + t * probe.w;
                int3 p = int3(floor((x - c->origin.xyz) / h));
                if (!inside(d, p)) { continue; }
                if (c->marker[cellIndex(d, p)] != FLUID_FLUID) { continue; }
                wet += 1;
                float vx, vy, vz;
                float3 row;
                gatherAxis(c, 0u, x, vx, row);
                gatherAxis(c, 1u, x, vy, row);
                gatherAxis(c, 2u, x, vz, row);
                vel += float3(vx, vy, vz);
            }
        }
    }

    int out0 = FLUID_MAX_PROBES + int(tid.x) * 2;
    float level = wetColumns > 0 ? surface / float(wetColumns) : -1e9;
    c->probes[out0] = float4(level, wet > 0 ? vel / float(wet) : float3(0.0));
    c->probes[out0 + 1] =
        float4(float(wetColumns) / float(RING), float(wet), 0.0, 0.0);
}

/// Recycle a block of particles back to the inflow box.
///
/// **A closed loop, and the choice of *which* particles is the whole design.**
/// The obvious inflow — birth at the lip, death at a drain — needs a free list,
/// which needs a compaction, which needs an atomic append, which is the exact
/// thing ADR 0053 §5 forbids on this path. Recycling by *ordinal* instead needs
/// none of it: at tick T the block `[start, start + n)` is teleported to the
/// lip, `start` advancing by `n` a tick and wrapping. It is the same arithmetic
/// ADR 0047 uses to give the GPU particle pool a free-list-free allocator, and
/// it is a pure function of (ordinal, tick).
///
/// **The particle count never changes, so the volume is exactly conserved.** A
/// particle leaving the pool for the lip is water pumped round; at the rates a
/// cascade authors, `n` is a tenth of a percent of the pool a tick, spread
/// uniformly over the ordinals and therefore uniformly over the pool, so
/// nothing anywhere reads as a hole.
///
/// `push.args.x` is `start`, computed on the CPU where the tick is a `u64`;
/// `push.args.y` is `n`. Both are zero on a domain with no cascade over it,
/// and the pass is then not recorded at all.
[numthreads(256, 1, 1)]
void fluidInflowMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    uint n = uint(c->dims.w);
    if (tid.x >= n) { return; }
    uint block = push.args.y;
    if (block == 0u) { return; }
    // Distance forward from this tick's first recycled ordinal, wrapping.
    uint rel = (tid.x + n - (push.args.x % n)) % n;
    if (rel >= block) { return; }

    // Jittered by the ordinal, so the lip is a sheet of water rather than a
    // lattice of points — and reproducibly, because `hash01` is arithmetic on
    // the index and there is no `thread_rng` anywhere near this.
    float3 f = float3(hash01(tid.x * 3u + 1u), hash01(tid.x * 3u + 2u), hash01(tid.x * 3u + 3u));
    FluidParticle p;
    p.position = float4(lerp(c->inflowLo.xyz, c->inflowHi.xyz, f), 0.0);
    p.velocity = float4(c->inflowVelocity.xyz, 0.0);
    // The affine state is reset with it: a parcel arriving at the brink carries
    // no memory of the pool it was pumped out of, and keeping its old velocity
    // gradient would spin it.
    p.c0 = float4(0.0);
    p.c1 = float4(0.0);
    p.c2 = float4(0.0);
    c->particles[tid.x] = p;
}

/// Fixed-point scale for the density splat below. See `fluidDensitySplatMain`.
static const uint FLUID_DENSITY_SCALE = 4096u;

/// Clear the density field the surface is extracted from.
[numthreads(256, 1, 1)]
void fluidDensityClearMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (int(tid.x) >= c->sizes.x) { return; }
    c->density[tid.x] = 0u;
}

/// Splat every particle into the density field with a trilinear kernel.
///
/// **This is not `cellCount`, and the difference is the whole surface.** A
/// count is a step function of the cell a particle happens to be in, so an
/// isosurface built on it is a staircase at the cell size — 10 cm on
/// `slosh.loom`, which reads as Minecraft water. A trilinear splat spreads each
/// particle over the eight cell centres around it, which is a one-cell-wide
/// filter applied at the source rather than a smoothing pass applied after, and
/// it is what makes the free surface move continuously as a particle crosses a
/// cell boundary instead of jumping.
///
/// **Fixed-point integer atomics, and that is ADR 0053 §5 in one line.** Float
/// `InterlockedAdd` would make this field depend on the order the scheduler
/// happened to run the particles in, and `cargo xtask repeat` compares PNGs
/// byte for byte. Integer addition is associative; the scale below is the
/// price, and 1/4096 of a particle is four orders below anything the isosurface
/// can express.
[numthreads(256, 1, 1)]
void fluidDensitySplatMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (tid.x >= uint(c->dims.w)) { return; }
    int3 d = c->dims.xyz;
    float h = c->origin.w;
    // Cell-centre lattice coordinates: 0.0 is the centre of cell 0.
    float3 g = (c->particles[tid.x].position.xyz - c->origin.xyz) / h - 0.5;
    int3 base = int3(floor(g));
    float3 f = g - float3(base);
    for (int k = 0; k < 2; ++k) {
        for (int j = 0; j < 2; ++j) {
            for (int i = 0; i < 2; ++i) {
                int3 p = base + int3(i, j, k);
                if (!inside(d, p)) { continue; }
                float w = (i == 0 ? 1.0 - f.x : f.x)
                        * (j == 0 ? 1.0 - f.y : f.y)
                        * (k == 0 ? 1.0 - f.z : f.z);
                uint q = uint(w * float(FLUID_DENSITY_SCALE));
                if (q == 0u) { continue; }
                uint ignored;
                InterlockedAdd(c->density[cellIndex(d, p)], q, ignored);
            }
        }
    }
}

/// Every `stride`-th particle, as a `ParticleInstance` the one particle
/// renderer already draws (ADR 0047's rule — there is no second one).
[numthreads(256, 1, 1)]
void fluidInstanceMain(uint3 tid: SV_DispatchThreadID) {
    FluidConsts* c = push.c;
    if (int(tid.x) >= c->counts.z) { return; }
    uint p = tid.x * uint(c->counts.y);
    float3 x = float3(0.0);
    float3 v = float3(0.0);
    float radius = 0.0;
    if (p < uint(c->dims.w)) {
        FluidParticle q = c->particles[p];
        x = q.position.xyz;
        v = q.velocity.xyz;
        // A droplet the size of the volume one drawn particle stands for, so
        // the field reads as water rather than as freckles.
        radius = c->origin.w * 0.42;
        // **Spray only, once there is a surface.** A particle in a cell at or
        // above half rest density is *inside* the marched mesh (ADR 0057
        // addendum: that fraction is the isovalue), so drawing it puts a blue
        // billboard over refracting water — which is what the whole surface
        // exists to stop. What is left is the thrown droplets, which is the
        // half of reference image 65 a mesh cannot express.
        //
        // Zero radius rather than a skipped slot: the instance stride is a CPU
        // fact and a degenerate quad is discarded before rasterisation, the
        // same trick the grass fade and the covered water rings use.
        int cell = particleCell(c, x);
        if (c->density[cell] >= FLUID_DENSITY_SCALE * FLUID_PER_CELL / 2u) {
            radius = 0.0;
        }
    }
    c->instances[tid.x * 3u] = float4(x, radius);
    c->instances[tid.x * 3u + 1u] = float4(0.20, 0.40, 0.56, 1.0);
    // The shutter is slice 2's: a droplet moving fast is a smear, which is
    // what turns a bead field into thrown water.
    c->instances[tid.x * 3u + 2u] = float4(v, 1.0 / 120.0);
}
