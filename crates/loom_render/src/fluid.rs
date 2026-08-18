//! The cinematic tier's fluid solver — ADR 0057, under ADR 0053.
//!
//! `assets/shaders/fluid_sim.slang` is the other half and carries the
//! reasoning about the algorithm; this file is the plumbing, plus the two
//! things the shader cannot do: the CPU rasterisation of the solids, and the
//! synchronous readback.
//!
//! # Why this owns a device
//!
//! ADR 0053 §5 keeps `ash` inside `loom_render*`, so the fixed step in
//! `loom_cli` cannot hold a Vulkan handle. It holds a [`FluidSolver`] instead,
//! whose whole surface is plain `f32`: [`FluidInputs`] in, [`FluidStepOutput`]
//! out. The solver therefore creates its own headless compute device — the
//! same `Device::new` the render path uses, with no surface and no swapchain —
//! rather than borrowing the renderer's, because `loom sim` has no renderer at
//! all and the alternative is threading a device handle through the CLI.
//!
//! # Barriers
//!
//! All of them belong to the render graph (never-do #4), exactly as `rain.rs`
//! does it. The solver records ~175 compute passes per tick into a
//! [`loom_render_graph::RenderGraph`] and submits them as **its own submit**,
//! separate from any frame — the precedent the TLAS rebuild sets.
//!
//! # The readback
//!
//! Synchronous and inside the fixed step (ADR 0053 §3). One `vkQueueSubmit`
//! and one fence wait per tick, so tick N reads the result of exactly N
//! dispatches. What is read back is **probes, not the grid**: a few hundred
//! bytes describing what the fluid is doing where a pontoon is.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator, AllocatorCreateDesc};
use gpu_allocator::MemoryLocation;
use loom_render_graph::{BufferAccess, BufferId, RenderGraph};

use crate::debug_names::DebugNames;
use crate::renderer::{
    create_address_buffer_in, create_shader_module, write_slice, ParticleInstance,
};
use crate::{Device, Instance};

/// Cells along the longest axis. The grid ceiling, and the divisor the cell
/// size is derived from — the scene authors an extent, never a cell.
pub const FLUID_GRID: usize = 64;

/// Particles seeded per filled cell.
pub const FLUID_PER_CELL: usize = 8;

/// Substeps per fixed tick. **Fixed, never adaptive** — an adaptive count
/// would make the dispatch sequence a function of the state rather than of the
/// tick, which is the property ADR 0053 §6 keeps.
const SUBSTEPS: u32 = 2;

/// V-cycles per projection, and smoothing sweeps per level. **Fixed, never
/// residual-tested**, for the same reason as the substeps: the dispatch
/// sequence must be a function of the tick alone.
///
/// **2 pre + 2 post was not enough and the failure was not subtle.** A settled
/// tank held its surface for forty ticks and then collapsed — 0.00 m at tick
/// 40, -0.04 at 80, -0.65 at 119, accelerating, until the water was packed
/// five times over at the bottom. An incompressible projection only removes
/// the divergent part of the velocity field; what it leaves behind at the free
/// surface, every substep, has nothing to push back against it, so an
/// under-converged solve does not jitter, it compresses. Sixty flat Jacobi
/// sweeps were worse still (-0.45 by tick 40), which is what said the problem
/// was convergence rather than the discretisation.
///
/// 4 pre + 4 post with twelve sweeps at the coarsest level holds the surface
/// at **exactly 0.0000** through 120 ticks, and costs *less* than the broken
/// version did — a collapsed tank has forty particles in a cell and the P2G
/// gather pays for every one of them.
const CYCLES: u32 = 3;

/// Damped-Jacobi sweeps before and after each restriction. See [`CYCLES`].
const SMOOTHS: u32 = 4;

/// Sweeps at the coarsest level, which stands in for an exact solve.
const COARSE_SMOOTHS: u32 = 12;

/// Multigrid levels, 64 down to 8.
const LEVELS: usize = 4;

/// Threads per group, mirroring `[numthreads(256,1,1)]`.
const GROUP: u32 = 256;

/// Instances the draw path is handed. Mirrors the renderer's own cap: the CPU
/// particle buffer holds 65,536 and sorts them, so writing more would be
/// truncated silently.
const MAX_INSTANCES: usize = 65_536;

/// The most pontoons one tick reads back.
const MAX_PROBES: usize = 64;

/// The cells a domain of this extent gets, and the metres one of them spans.
///
/// **Public because the caller bakes the static solid mask**, and has to size
/// it the same way the solver does. `loom_cli` knows what a voxel volume and a
/// `BoxCollider` are; this crate deliberately does not.
#[must_use]
pub fn fluid_grid(extent: [f32; 3]) -> ([usize; 3], f32) {
    let longest = extent.iter().copied().fold(0.0_f32, f32::max).max(1e-3);
    #[allow(clippy::cast_precision_loss)]
    let cell = longest / FLUID_GRID as f32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let axis = |e: f32| ((e / cell).round() as usize).clamp(4, FLUID_GRID);
    ([axis(extent[0]), axis(extent[1]), axis(extent[2])], cell)
}

/// The domain, anchored to the scene node that authored it.
///
/// **Never to the camera** — ADR 0053 §5. There is no eye in this struct and
/// there is deliberately nowhere to put one.
#[derive(Debug, Clone, Copy)]
pub struct FluidDomain {
    /// World centre of the box.
    pub centre: [f32; 3],
    /// Full size in metres.
    pub extent: [f32; 3],
    /// Fraction of the domain's height that starts full.
    pub fill: f32,
    /// The cascade pouring into it, if the scene authors one over cinematic
    /// water. `None` is a closed tank.
    pub inflow: Option<FluidInflow>,
}

/// A cascade pouring into the domain — ADR 0057 addendum.
///
/// **A closed loop, not a source and a sink.** `per_tick` particles are
/// recycled to the box every tick, chosen round-robin by ordinal, so the
/// particle count is constant and the volume is exactly conserved. See
/// `fluidInflowMain` for why an ordinal is the only free-list-free choice
/// available under ADR 0053 §5.
#[derive(Debug, Clone, Copy)]
pub struct FluidInflow {
    /// The lip box, world minimum corner.
    pub lo: [f32; 3],
    /// The lip box, world maximum corner.
    pub hi: [f32; 3],
    /// What the water leaves the brink at, m/s.
    pub velocity: [f32; 3],
    /// Particles recycled per fixed tick — the discharge, in the only units
    /// this solver has. Zero is no cascade, and the pass is not recorded.
    pub per_tick: u32,
}

/// One solid the fluid sees this tick: a box or a ball, and how fast it moves.
#[derive(Debug, Clone, Copy)]
pub struct FluidSolid {
    pub centre: [f32; 3],
    /// Half extents for a box; ignored for a ball.
    pub half: [f32; 3],
    /// Radius for a ball; ignored for a box.
    pub radius: f32,
    pub ball: bool,
    pub velocity: [f32; 3],
}

/// One pontoon the fluid is asked about.
#[derive(Debug, Clone, Copy)]
pub struct FluidProbe {
    pub at: [f32; 3],
    pub radius: f32,
}

/// Everything the fixed step tells the solver about this tick.
pub struct FluidInputs<'a> {
    /// Dynamic bodies, rasterised into the solid mask over the static bake.
    pub solids: &'a [FluidSolid],
    /// Pontoons to read back, in the order the caller wants them returned.
    pub probes: &'a [FluidProbe],
}

/// What the fluid is doing at one pontoon.
#[derive(Debug, Clone, Copy, Default)]
pub struct FluidProbeResult {
    /// World Y of the free surface **beside** this pontoon, or a large
    /// negative number where the water does not reach. See `fluidProbeMain`
    /// for why it is beside rather than through.
    pub surface: f32,
    /// Mean fluid velocity over the wetted part, m/s.
    pub velocity: [f32; 3],
    /// Fraction of the ring of columns that found water — 0 when the pontoon
    /// is outside the domain or over dry ground.
    pub wetness: f32,
}

/// What comes back across the boundary, as plain `f32`.
#[derive(Debug, Default)]
pub struct FluidStepOutput {
    pub probes: Vec<FluidProbeResult>,
    /// Milliseconds this tick spent waiting on the fence — the device→host
    /// sync ADR 0053 §4 asks to be measured and reported, not assumed.
    pub fence_wait_ms: f64,
    /// Milliseconds for the whole step, CPU rasterisation included.
    pub step_ms: f64,
}

/// One multigrid level, mirroring `FluidLevel`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Level {
    dim: [i32; 4],
    misc: [f32; 4],
}

/// Mirrors `FluidConsts`. Offsets pinned by a test against the compiled module.
#[repr(C)]
#[derive(Clone, Copy)]
struct Consts {
    origin: [f32; 4],
    dims: [i32; 4],
    timing: [f32; 4],
    sizes: [i32; 4],
    counts: [i32; 4],
    levels: [Level; LEVELS],
    particles: vk::DeviceAddress,
    cell_count: vk::DeviceAddress,
    cell_start: vk::DeviceAddress,
    cell_cursor: vk::DeviceAddress,
    sorted: vk::DeviceAddress,
    reordered: vk::DeviceAddress,
    block_sums: vk::DeviceAddress,
    vel_u: vk::DeviceAddress,
    vel_v: vk::DeviceAddress,
    vel_w: vk::DeviceAddress,
    marker: vk::DeviceAddress,
    pressure_a: vk::DeviceAddress,
    pressure_b: vk::DeviceAddress,
    rhs: vk::DeviceAddress,
    solid: vk::DeviceAddress,
    probes: vk::DeviceAddress,
    instances: vk::DeviceAddress,
    density: vk::DeviceAddress,
    inflow_lo: [f32; 4],
    inflow_hi: [f32; 4],
    inflow_velocity: [f32; 4],
}

/// Mirrors `FluidPush`. 32 bytes of the 128 guaranteed.
///
/// **The eight bytes of padding are load-bearing and cost a night.** A `uint4`
/// aligns to sixteen in std430, so Slang puts `args` at offset 16 while a
/// naive Rust `{u64, [u32;4]}` puts it at 8. Every dispatch then read its
/// level, axis and parity out of the eight bytes past the end of a 24-byte
/// push block — which the driver serves as zero rather than faulting. The
/// symptom was a tank of water that held its shape perfectly and never moved:
/// `axis` was always 0, so `if (axis == 1) v += gravity * dt` never fired, and
/// a fluid with no gravity is a lattice. `spirv-dis | grep 'OpMemberDecorate
/// %FluidPush'` is the check, and the test below is it pinned.
#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    consts: vk::DeviceAddress,
    /// See the note above. Never remove this.
    pad: u64,
    args: [u32; 4],
}

impl Push {
    fn bytes(&self) -> &[u8] {
        // SAFETY: `Push` is `repr(C)` and plain old data.
        unsafe {
            std::slice::from_raw_parts(std::ptr::from_ref(self).cast::<u8>(), size_of::<Self>())
        }
    }
}

/// A buffer the solver owns, kept together so teardown is one loop.
struct Buf {
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    address: vk::DeviceAddress,
    /// Whether the first submit zeroes it. See `fluid_zero`.
    zero: bool,
}

/// The twenty compute pipelines, one per entry point.
struct Pipelines {
    seed: vk::Pipeline,
    clear_counts: vk::Pipeline,
    count: vk::Pipeline,
    scan_block: vk::Pipeline,
    scan_sums: vk::Pipeline,
    scan_add: vk::Pipeline,
    scatter: vk::Pipeline,
    sort_cell: vk::Pipeline,
    reorder: vk::Pipeline,
    p2g: vk::Pipeline,
    forces: vk::Pipeline,
    marker: vk::Pipeline,
    restrict_marker: vk::Pipeline,
    divergence: vk::Pipeline,
    jacobi: vk::Pipeline,
    restrict: vk::Pipeline,
    prolong: vk::Pipeline,
    project: vk::Pipeline,
    g2p: vk::Pipeline,
    probe: vk::Pipeline,
    instance: vk::Pipeline,
    density_clear: vk::Pipeline,
    density_splat: vk::Pipeline,
    inflow: vk::Pipeline,
}

/// The solver, and the whole of `loom_cli`'s view of Vulkan.
pub struct FluidSolver {
    // Declared in drop order: the allocator must outlive nothing, and the
    // instance must outlive the device.
    bufs: Vec<Buf>,
    consts_alloc: Option<Allocation>,
    solid_alloc: Option<Allocation>,
    probes_alloc: Option<Allocation>,
    instances_alloc: Option<Allocation>,
    density_alloc: Option<Allocation>,
    /// The fluid fraction the last [`Self::density`] read back, one per cell.
    density: Vec<f32>,
    pipelines: Pipelines,
    layout: vk::PipelineLayout,
    module: vk::ShaderModule,
    pool: vk::CommandPool,
    allocator: Option<Allocator>,
    device: Device,
    _instance: Instance,

    dims: [usize; 3],
    cell: f32,
    origin: [f32; 3],
    particles: usize,
    levels: [Level; LEVELS],
    consts: Consts,

    /// Particles the cascade recycles to its lip every tick. Zero is a closed
    /// tank, and the inflow pass is then not recorded at all.
    inflow_per_tick: u32,
    /// The static solid mask, rasterised once by the caller. The dynamic
    /// bodies are drawn over a copy of it every tick.
    static_solid: Vec<[f32; 4]>,
    scratch_solid: Vec<[f32; 4]>,

    seeded: bool,
    tick: u64,
    out: FluidStepOutput,
}

impl FluidSolver {
    /// Build the solver for one domain.
    ///
    /// `static_solid` is one byte per cell, non-zero where the world is solid,
    /// in `x + nx*(y + ny*z)` order — the caller's bake of the voxel SDF and
    /// the static box colliders.
    ///
    /// # Errors
    /// A string naming what failed, so `loom sim` can refuse loudly.
    pub fn new(domain: FluidDomain, static_solid: &[u8]) -> Result<Self, String> {
        let (dims, cell) = fluid_grid(domain.extent);
        let cells = dims[0] * dims[1] * dims[2];
        if static_solid.len() != cells {
            return Err(format!(
                "the static solid mask is {} cells and the domain is {cells}",
                static_solid.len()
            ));
        }

        let instance = Instance::new(c"loom").map_err(|e| e.to_string())?;
        let device = Device::new(&instance).map_err(|e| e.to_string())?;
        let raw = device.handle().clone();
        let names = DebugNames::new(instance.handle(), &raw, cfg!(debug_assertions));

        let mut allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.handle().clone(),
            device: raw.clone(),
            physical_device: device.physical(),
            debug_settings: gpu_allocator::AllocatorDebugSettings::default(),
            buffer_device_address: true,
            allocation_sizes: gpu_allocator::AllocationSizes::default(),
        })
        .map_err(|e| e.to_string())?;

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let filled = ((domain.fill.clamp(0.0, 1.0) * dims[1] as f32).round() as usize).min(dims[1]);
        let particles = dims[0] * filled * dims[2] * FLUID_PER_CELL;
        if particles == 0 {
            return Err("a cinematic domain with fill = 0 has no fluid in it".to_owned());
        }

        // The level table. Each level halves, floored at one cell.
        let mut levels = [Level::default(); LEVELS];
        let mut base = 0_usize;
        for (l, level) in levels.iter_mut().enumerate() {
            let d = [
                (dims[0] >> l).max(1),
                (dims[1] >> l).max(1),
                (dims[2] >> l).max(1),
            ];
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            {
                level.dim = [d[0] as i32, d[1] as i32, d[2] as i32, base as i32];
            }
            #[allow(clippy::cast_precision_loss)]
            {
                level.misc = [cell * (1_u32 << l) as f32, 0.0, 0.0, 0.0];
            }
            base += d[0] * d[1] * d[2];
        }
        let level_cells = base;

        let blocks = cells.div_ceil(GROUP as usize);
        if blocks > 1024 {
            return Err(format!(
                "{cells} cells needs {blocks} scan blocks and `fluidScanSumsMain` \
                 scans at most 1024"
            ));
        }

        let u = (dims[0] + 1) * dims[1] * dims[2];
        let v = dims[0] * (dims[1] + 1) * dims[2];
        let w = dims[0] * dims[1] * (dims[2] + 1);

        let mut bufs = Vec::new();
        let mut make = |size: usize, name: &'static str, loc: MemoryLocation| {
            create_address_buffer_in(
                &raw,
                &mut allocator,
                size as u64,
                name,
                // `TRANSFER_DST` because the first submit zeroes every
                // device-local buffer with `vkCmdFillBuffer` — see `fluid_zero`.
                // The validation layers catch the omission, and did.
                vk::BufferUsageFlags::TRANSFER_DST,
                loc,
            )
            .map_err(|e| e.to_string())
            .map(|(buffer, allocation, address)| {
                names.set(buffer, name);
                (buffer, allocation, address)
            })
        };

        let gpu = MemoryLocation::GpuOnly;
        let (pb, pa, particles_addr) = make(particles * 80, "loom.fluid.particles", gpu)?;
        let (ccb, cca, cell_count) = make(cells * 4, "loom.fluid.cell_count", gpu)?;
        let (csb, csa, cell_start) = make(cells * 4, "loom.fluid.cell_start", gpu)?;
        let (cub, cua, cell_cursor) = make(cells * 4, "loom.fluid.cell_cursor", gpu)?;
        let (sob, soa, sorted) = make(particles * 4, "loom.fluid.sorted", gpu)?;
        let (rob, roa, reordered) = make(particles * 80, "loom.fluid.reordered", gpu)?;
        let (bsb, bsa, block_sums) = make(blocks.max(1) * 4, "loom.fluid.block_sums", gpu)?;
        let (ub, ua, vel_u) = make(u * 4, "loom.fluid.vel_u", gpu)?;
        let (vb, va, vel_v) = make(v * 4, "loom.fluid.vel_v", gpu)?;
        let (wb, wa, vel_w) = make(w * 4, "loom.fluid.vel_w", gpu)?;
        let (mb, ma, marker) = make(level_cells * 4, "loom.fluid.marker", gpu)?;
        let (pab, paa, pressure_a) = make(level_cells * 4, "loom.fluid.pressure_a", gpu)?;
        let (pbb, pba, pressure_b) = make(level_cells * 4, "loom.fluid.pressure_b", gpu)?;
        let (rb, ra, rhs) = make(level_cells * 4, "loom.fluid.rhs", gpu)?;
        // Host-visible: the CPU rewrites the solid mask every tick, and the
        // fence wait before it means there is nothing to double-buffer.
        let (sdb, sda, solid) = make(cells * 16, "loom.fluid.solid", MemoryLocation::CpuToGpu)?;
        let (prb, pra, probes) = make(
            (MAX_PROBES * 3) * 16,
            "loom.fluid.probes",
            MemoryLocation::CpuToGpu,
        )?;
        let (inb, ina, instances) = make(
            MAX_INSTANCES * size_of::<ParticleInstance>(),
            "loom.fluid.instances",
            MemoryLocation::GpuToCpu,
        )?;
        let (cnb, cna, consts_addr) =
            make(size_of::<Consts>(), "loom.fluid.consts", MemoryLocation::CpuToGpu)?;
        // Host-visible: the surface is marched on the CPU, one read per frame
        // drawn rather than one per tick. See `FluidSolver::density`.
        let (deb, dea, density) = make(cells * 4, "loom.fluid.density", MemoryLocation::GpuToCpu)?;

        let inflow = domain.inflow.unwrap_or(FluidInflow {
            lo: [0.0; 3],
            hi: [0.0; 3],
            velocity: [0.0; 3],
            per_tick: 0,
        });
        let (lo, hi, flux) = (inflow.lo, inflow.hi, inflow.velocity);
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let consts = Consts {
            origin: [
                domain.centre[0] - domain.extent[0] * 0.5,
                domain.centre[1] - domain.extent[1] * 0.5,
                domain.centre[2] - domain.extent[2] * 0.5,
                cell,
            ],
            dims: [dims[0] as i32, dims[1] as i32, dims[2] as i32, particles as i32],
            timing: [
                1.0 / (60.0 * f32::from(u8::try_from(SUBSTEPS).unwrap_or(2))),
                -9.81,
                // **The CFL guard**, and the reason substeps are fixed at two:
                // a particle may not cross more than 0.9 of a cell in one
                // substep, or the P2G stencil it lands in has no relation to
                // the one it left.
                0.9 * cell * 60.0 * f32::from(u8::try_from(SUBSTEPS).unwrap_or(2)),
                0.0,
            ],
            sizes: [cells as i32, u as i32, v as i32, w as i32],
            counts: [
                level_cells as i32,
                (particles.div_ceil(MAX_INSTANCES)).max(1) as i32,
                particles.div_ceil(particles.div_ceil(MAX_INSTANCES).max(1)).min(MAX_INSTANCES)
                    as i32,
                0,
            ],
            levels,
            particles: particles_addr,
            cell_count,
            cell_start,
            cell_cursor,
            sorted,
            reordered,
            block_sums,
            vel_u,
            vel_v,
            vel_w,
            marker,
            pressure_a,
            pressure_b,
            rhs,
            solid,
            probes,
            instances,
            density,
            inflow_lo: [lo[0], lo[1], lo[2], 0.0],
            inflow_hi: [hi[0], hi[1], hi[2], 0.0],
            inflow_velocity: [flux[0], flux[1], flux[2], 0.0],
        };

        let range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(u32::try_from(size_of::<Push>()).unwrap_or(32));
        let ranges = [range];
        let layout_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&ranges);
        // SAFETY: `ranges` outlives the call.
        let layout = unsafe { raw.create_pipeline_layout(&layout_info, None) }
            .map_err(|e| e.to_string())?;
        names.set(layout, "loom.fluid.layout");

        let module = create_shader_module(&raw, crate::FLUID_SIM_SPV).map_err(|e| e.to_string())?;
        let make_pipe = |entry: &std::ffi::CStr, name: &str| -> Result<vk::Pipeline, String> {
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(entry);
            let info = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(layout);
            // SAFETY: the module and layout are live, and `info` outlives the
            // call.
            let pipeline = unsafe {
                raw.create_compute_pipelines(vk::PipelineCache::null(), &[info], None)
            }
            .map_err(|(_, e)| e.to_string())?[0];
            names.set(pipeline, name);
            Ok(pipeline)
        };

        let pipelines = Pipelines {
            seed: make_pipe(c"fluidSeedMain", "loom.fluid.seed")?,
            clear_counts: make_pipe(c"fluidClearCountsMain", "loom.fluid.clear_counts")?,
            count: make_pipe(c"fluidCountMain", "loom.fluid.count")?,
            scan_block: make_pipe(c"fluidScanBlockMain", "loom.fluid.scan_block")?,
            scan_sums: make_pipe(c"fluidScanSumsMain", "loom.fluid.scan_sums")?,
            scan_add: make_pipe(c"fluidScanAddMain", "loom.fluid.scan_add")?,
            scatter: make_pipe(c"fluidScatterMain", "loom.fluid.scatter")?,
            sort_cell: make_pipe(c"fluidSortCellMain", "loom.fluid.sort_cell")?,
            reorder: make_pipe(c"fluidReorderMain", "loom.fluid.reorder")?,
            p2g: make_pipe(c"fluidP2GMain", "loom.fluid.p2g")?,
            forces: make_pipe(c"fluidForcesMain", "loom.fluid.forces")?,
            marker: make_pipe(c"fluidMarkerMain", "loom.fluid.marker")?,
            restrict_marker: make_pipe(c"fluidRestrictMarkerMain", "loom.fluid.restrict_marker")?,
            divergence: make_pipe(c"fluidDivergenceMain", "loom.fluid.divergence")?,
            jacobi: make_pipe(c"fluidJacobiMain", "loom.fluid.jacobi")?,
            restrict: make_pipe(c"fluidRestrictMain", "loom.fluid.restrict")?,
            prolong: make_pipe(c"fluidProlongMain", "loom.fluid.prolong")?,
            project: make_pipe(c"fluidProjectMain", "loom.fluid.project")?,
            g2p: make_pipe(c"fluidG2PMain", "loom.fluid.g2p")?,
            probe: make_pipe(c"fluidProbeMain", "loom.fluid.probe")?,
            instance: make_pipe(c"fluidInstanceMain", "loom.fluid.instance")?,
            inflow: make_pipe(c"fluidInflowMain", "loom.fluid.inflow")?,
            density_clear: make_pipe(c"fluidDensityClearMain", "loom.fluid.density_clear")?,
            density_splat: make_pipe(c"fluidDensitySplatMain", "loom.fluid.density_splat")?,
        };

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(device.queue_family())
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the family index came from this device.
        let pool = unsafe { raw.create_command_pool(&pool_info, None) }.map_err(|e| e.to_string())?;
        names.set(pool, "loom.fluid.pool");

        let solid_base: Vec<[f32; 4]> = static_solid
            .iter()
            .map(|s| [0.0, 0.0, 0.0, if *s == 0 { 0.0 } else { 1.0 }])
            .collect();

        let bufs_list = [
            (pb, pa, particles_addr),
            (ccb, cca, cell_count),
            (csb, csa, cell_start),
            (cub, cua, cell_cursor),
            (sob, soa, sorted),
            (bsb, bsa, block_sums),
            (ub, ua, vel_u),
            (vb, va, vel_v),
            (wb, wa, vel_w),
            (mb, ma, marker),
            (pab, paa, pressure_a),
            (pbb, pba, pressure_b),
            (rb, ra, rhs),
            (sdb, sda, solid),
            (prb, pra, probes),
            (inb, ina, instances),
            (cnb, cna, consts_addr),
            (rob, roa, reordered),
            (deb, dea, density),
        ];
        let mut solid_alloc = None;
        let mut probes_alloc = None;
        let mut instances_alloc = None;
        let mut consts_alloc = None;
        let mut density_alloc = None;
        for (index, (buffer, allocation, address)) in bufs_list.into_iter().enumerate() {
            // **Which buffers the first submit zeroes, and the host-visible
            // ones are not among them.** `fluid_zero` runs inside the first
            // command buffer, after the CPU has already written the constants
            // and the solid mask into their mappings — filling those would
            // erase them.
            let keep = match index {
                13 => &mut solid_alloc,
                14 => &mut probes_alloc,
                15 => &mut instances_alloc,
                16 => &mut consts_alloc,
                18 => &mut density_alloc,
                _ => {
                    bufs.push(Buf { buffer, allocation: Some(allocation), address, zero: true });
                    continue;
                }
            };
            *keep = Some(allocation);
            bufs.push(Buf { buffer, allocation: None, address, zero: false });
        }

        let mut solver = Self {
            bufs,
            consts_alloc,
            solid_alloc,
            probes_alloc,
            instances_alloc,
            density_alloc,
            density: vec![0.0; cells],
            pipelines,
            layout,
            module,
            pool,
            allocator: Some(allocator),
            device,
            _instance: instance,
            dims,
            cell,
            origin: [consts.origin[0], consts.origin[1], consts.origin[2]],
            particles,
            levels,
            consts,
            inflow_per_tick: inflow.per_tick,
            static_solid: solid_base.clone(),
            scratch_solid: solid_base,
            seeded: false,
            tick: 0,
            out: FluidStepOutput::default(),
        };
        solver.write_consts()?;
        // Zeroed once, so that `fluidInstanceMain`'s spray cull reads "no
        // surface here" rather than whatever the allocator handed back, on any
        // path that draws particles before ever marching a surface.
        if let Some(alloc) = solver.density_alloc.as_ref() {
            let _ = write_slice(alloc, &vec![0_u32; cells]);
        }
        Ok(solver)
    }

    fn write_consts(&mut self) -> Result<(), String> {
        let alloc = self
            .consts_alloc
            .as_ref()
            .ok_or_else(|| "the fluid constant block is gone".to_owned())?;
        write_slice(alloc, std::slice::from_ref(&self.consts)).map_err(|e| e.to_string())
    }

    /// Particles the solver carries — constant for the life of the domain.
    #[must_use]
    pub fn particle_count(&self) -> usize {
        self.particles
    }

    /// Cells per axis, and metres per cell.
    #[must_use]
    pub fn grid(&self) -> ([usize; 3], f32) {
        (self.dims, self.cell)
    }

    /// The domain's world-space bounds, so the caller can decide which bodies
    /// are in the tier and which are not (ADR 0053 §5 — the tier does not leak).
    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        #[allow(clippy::cast_precision_loss)]
        let hi = [
            self.origin[0] + self.dims[0] as f32 * self.cell,
            self.origin[1] + self.dims[1] as f32 * self.cell,
            self.origin[2] + self.dims[2] as f32 * self.cell,
        ];
        (self.origin, hi)
    }

    /// Advance one fixed tick and read the probes back.
    ///
    /// **Synchronous, inside the caller's fixed step.** ADR 0053 §3 forbids an
    /// asynchronous or frame-delayed readback: it would lose the tick ordering
    /// that the whole tier is allowed to exist on.
    pub fn step(&mut self, inputs: &FluidInputs) -> &FluidStepOutput {
        // **Never-do #8 says simulation must not read the wall clock, and this
        // does not.** The clock is read for instrumentation only: ADR 0053 §4
        // requires the device round trip to be measured and reported rather
        // than assumed tolerable, and nothing in the solve, the forces or the
        // dispatch sequence reads either number back.
        #[allow(clippy::disallowed_methods)]
        let started = std::time::Instant::now();
        self.rasterise(inputs.solids);
        self.write_probes(inputs.probes);

        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        {
            self.consts.counts[3] = inputs.probes.len().min(MAX_PROBES) as i32;
        }
        let _ = self.write_consts();

        let waited = self.submit();
        self.read_probes(inputs.probes.len().min(MAX_PROBES));
        self.tick += 1;
        self.out.fence_wait_ms = waited;
        self.out.step_ms = started.elapsed().as_secs_f64() * 1000.0;
        &self.out
    }

    /// Draw the dynamic bodies into the solid mask over the static bake.
    ///
    /// On the CPU, from rapier transforms — a fact of (scene, tick), which is
    /// what keeps the coupling's *input* deterministic even though its output
    /// is not.
    fn rasterise(&mut self, solids: &[FluidSolid]) {
        self.scratch_solid.copy_from_slice(&self.static_solid);
        let h = self.cell;
        for s in solids {
            let reach = if s.ball {
                [s.radius, s.radius, s.radius]
            } else {
                s.half
            };
            let mut lo = [0_usize; 3];
            let mut hi = [0_usize; 3];
            for a in 0..3 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let l = ((s.centre[a] - reach[a] - self.origin[a]) / h).floor().max(0.0) as usize;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let u = ((s.centre[a] + reach[a] - self.origin[a]) / h).ceil().max(0.0) as usize;
                lo[a] = l.min(self.dims[a]);
                hi[a] = u.min(self.dims[a]);
            }
            for k in lo[2]..hi[2] {
                for j in lo[1]..hi[1] {
                    for i in lo[0]..hi[0] {
                        #[allow(clippy::cast_precision_loss)]
                        let p = [
                            self.origin[0] + (i as f32 + 0.5) * h,
                            self.origin[1] + (j as f32 + 0.5) * h,
                            self.origin[2] + (k as f32 + 0.5) * h,
                        ];
                        let inside = if s.ball {
                            let d = [
                                p[0] - s.centre[0],
                                p[1] - s.centre[1],
                                p[2] - s.centre[2],
                            ];
                            d[0].mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2]))
                                <= s.radius * s.radius
                        } else {
                            (0..3).all(|a| (p[a] - s.centre[a]).abs() <= reach[a])
                        };
                        if inside {
                            let index = i + self.dims[0] * (j + self.dims[1] * k);
                            self.scratch_solid[index] =
                                [s.velocity[0], s.velocity[1], s.velocity[2], 1.0];
                        }
                    }
                }
            }
        }
        if let Some(alloc) = self.solid_alloc.as_ref() {
            let _ = write_slice(alloc, &self.scratch_solid);
        }
    }

    fn write_probes(&self, probes: &[FluidProbe]) {
        let mut block = [[0.0_f32; 4]; MAX_PROBES];
        for (slot, probe) in block.iter_mut().zip(probes.iter().take(MAX_PROBES)) {
            *slot = [probe.at[0], probe.at[1], probe.at[2], probe.radius];
        }
        if let Some(alloc) = self.probes_alloc.as_ref() {
            let _ = write_slice(alloc, &block);
        }
    }

    fn read_probes(&mut self, count: usize) {
        self.out.probes.clear();
        let Some(alloc) = self.probes_alloc.as_ref() else { return };
        let Some(ptr) = alloc.mapped_ptr() else { return };
        for index in 0..count {
            // SAFETY: the buffer holds `MAX_PROBES * 3` `float4`s, the shader
            // wrote two per probe starting at `MAX_PROBES`, and the device has
            // been fence-waited.
            let (a, b) = unsafe {
                let base = ptr.as_ptr().cast::<[f32; 4]>();
                (
                    base.add(MAX_PROBES + index * 2).read_unaligned(),
                    base.add(MAX_PROBES + index * 2 + 1).read_unaligned(),
                )
            };
            self.out.probes.push(FluidProbeResult {
                surface: a[0],
                velocity: [a[1], a[2], a[3]],
                wetness: b[0],
            });
        }
    }

    /// Read the particles back as draw instances.
    ///
    /// **Every `stride`-th particle, never a decimation the camera chooses** —
    /// the stride is `ceil(N / 65,536)`, a function of the domain alone, so two
    /// renders of the same tick draw the same droplets.
    #[must_use]
    pub fn instances(&mut self) -> Vec<ParticleInstance> {
        let count = usize::try_from(self.consts.counts[2]).unwrap_or(0);
        if count == 0 {
            return Vec::new();
        }
        let pipeline = self.pipelines.instance;
        let layout = self.layout;
        let push = Push { consts: self.consts_address(), pad: 0, args: [0; 4] };
        #[allow(clippy::cast_possible_truncation)]
        let groups = (count as u32).div_ceil(GROUP);
        let particles = self.bufs[0].buffer;
        let instances = self.bufs[15].buffer;
        let device = self.device.handle().clone();
        let queue = self.device.queue();
        let pool = self.pool;
        let _ = crate::material::record(&device, crate::raytrace::Submit { pool, queue }, |cmd| {
            let mut graph = RenderGraph::new();
            let p = graph.import_buffer("loom.fluid.particles", particles);
            let i = graph.import_buffer("loom.fluid.instances", instances);
            graph.pass_with(
                "fluid_instances",
                &[],
                &[(p, BufferAccess::ComputeRead), (i, BufferAccess::ComputeReadWrite)],
                move |d, cmd| record_dispatch(d, cmd, layout, pipeline, push, groups),
            );
            graph.execute(&device, cmd);
        });

        let mut out = vec![
            ParticleInstance {
                position: [0.0; 4],
                color: [0.0; 4],
                velocity: [0.0; 4],
            };
            count
        ];
        if let Some(ptr) = self.instances_alloc.as_ref().and_then(Allocation::mapped_ptr) {
            // SAFETY: the buffer holds `MAX_INSTANCES` instances, `count` is at
            // most that, and the dispatch above was fence-waited by `record`.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    ptr.as_ptr().cast::<ParticleInstance>(),
                    out.as_mut_ptr(),
                    count,
                );
            }
        }
        out
    }

    /// The fluid fraction in every cell, 1.0 at rest density.
    ///
    /// **Read back, and that is a divergence from ADR 0057's plan worth
    /// stating.** The intended shape was marching cubes on the device with the
    /// vertex buffer never leaving it. It cannot be: this solver holds a
    /// **compute device of its own** (see the module header — `loom_cli` has no
    /// renderer at all and `loom sim` has no window), and a buffer on that
    /// device is not something the renderer's device can draw. So the surface
    /// crosses as plain `f32` through the same narrow interface the probes do,
    /// and the marching happens in [`crate::fluid_surface`] on the CPU.
    ///
    /// What that costs is one 128 KB copy per frame *drawn* — not per tick —
    /// and a CPU march. What it buys is that the extraction has no atomic and
    /// no scan in it at all: a loop over cells in index order emits triangles
    /// in index order, which is a stronger reproducibility guarantee than the
    /// scan-based compaction ADR 0057 was going to need, in a tenth of the
    /// code.
    ///
    /// Two dispatches: clear, then splat. The count the solver already keeps
    /// is not usable here — see `fluidDensitySplatMain`.
    pub fn density(&mut self) -> &[f32] {
        let cells = self.dims[0] * self.dims[1] * self.dims[2];
        let push = Push { consts: self.consts_address(), pad: 0, args: [0; 4] };
        #[allow(clippy::cast_possible_truncation)]
        let cell_groups = (cells as u32).div_ceil(GROUP);
        #[allow(clippy::cast_possible_truncation)]
        let particle_groups = (self.particles as u32).div_ceil(GROUP);
        let (clear, splat) = (self.pipelines.density_clear, self.pipelines.density_splat);
        let layout = self.layout;
        let particles = self.bufs[0].buffer;
        let density = self.bufs[18].buffer;
        let device = self.device.handle().clone();
        let queue = self.device.queue();
        let pool = self.pool;
        let _ = crate::material::record(&device, crate::raytrace::Submit { pool, queue }, |cmd| {
            let mut graph = RenderGraph::new();
            let p = graph.import_buffer("loom.fluid.particles", particles);
            let g = graph.import_buffer("loom.fluid.density", density);
            graph.pass_with(
                "fluid_density_clear",
                &[],
                &[(g, BufferAccess::ComputeReadWrite)],
                move |d, cmd| record_dispatch(d, cmd, layout, clear, push, cell_groups),
            );
            graph.pass_with(
                "fluid_density_splat",
                &[],
                &[(p, BufferAccess::ComputeRead), (g, BufferAccess::ComputeReadWrite)],
                move |d, cmd| record_dispatch(d, cmd, layout, splat, push, particle_groups),
            );
            graph.execute(&device, cmd);
        });

        if let Some(ptr) = self.density_alloc.as_ref().and_then(Allocation::mapped_ptr) {
            #[allow(clippy::cast_precision_loss)]
            let scale = 1.0 / (4096.0 * FLUID_PER_CELL as f32);
            for (slot, index) in self.density.iter_mut().zip(0..cells) {
                // SAFETY: the buffer holds `cells` `u32`s and the dispatches
                // above were fence-waited by `record`.
                let raw = unsafe { ptr.as_ptr().cast::<u32>().add(index).read_unaligned() };
                #[allow(clippy::cast_precision_loss)]
                {
                    *slot = raw as f32 * scale;
                }
            }
        }
        &self.density
    }

    fn consts_address(&self) -> vk::DeviceAddress {
        self.bufs[16].address
    }

    /// Record and submit this tick's dispatches, and wait for them.
    ///
    /// Returns the milliseconds spent in the fence wait.
    #[allow(clippy::too_many_lines)]
    fn submit(&mut self) -> f64 {
        let device = self.device.handle().clone();
        let queue = self.device.queue();
        let submit = crate::raytrace::Submit { pool: self.pool, queue };
        let consts = self.consts_address();
        let p = &self.pipelines;
        let layout = self.layout;
        let seed = !self.seeded;
        self.seeded = true;

        let cells = self.dims[0] * self.dims[1] * self.dims[2];
        #[allow(clippy::cast_possible_truncation)]
        let particle_groups = (self.particles as u32).div_ceil(GROUP);
        #[allow(clippy::cast_possible_truncation)]
        let cell_groups = (cells as u32).div_ceil(GROUP);
        let face_groups: [u32; 3] = {
            let d = self.dims;
            #[allow(clippy::cast_possible_truncation)]
            [
                (((d[0] + 1) * d[1] * d[2]) as u32).div_ceil(GROUP),
                ((d[0] * (d[1] + 1) * d[2]) as u32).div_ceil(GROUP),
                ((d[0] * d[1] * (d[2] + 1)) as u32).div_ceil(GROUP),
            ]
        };
        #[allow(clippy::cast_possible_truncation)]
        let extended_groups = (((self.dims[0] + 1) * (self.dims[1] + 1) * (self.dims[2] + 1))
            as u32)
            .div_ceil(GROUP);
        let mut level_groups = [0_u32; LEVELS];
        for (g, level) in level_groups.iter_mut().zip(self.levels.iter()) {
            #[allow(clippy::cast_sign_loss)]
            let n = (level.dim[0] * level.dim[1] * level.dim[2]) as u32;
            *g = n.div_ceil(GROUP);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let probe_groups = self.consts.counts[3].max(0) as u32;

        let handles: Vec<vk::Buffer> = self.bufs.iter().map(|b| b.buffer).collect();
        let zeroed: Vec<bool> = self.bufs.iter().map(|b| b.zero).collect();

        // **Not `material::record`**, which submits and waits in one call. The
        // fence wait is the number ADR 0053 §4 asks for by name, so it has to
        // be separable from recording the 175 dispatches around it.
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool belongs to this device.
        let Ok(buffers) = (unsafe { device.allocate_command_buffers(&allocate) }) else {
            return 0.0;
        };
        let cmd = buffers[0];
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: `cmd` was just allocated and is not recording.
        let _ = unsafe { device.begin_command_buffer(cmd, &begin) };

        {
            let mut graph = RenderGraph::new();
            let ids: Vec<BufferId> = handles
                .iter()
                .map(|b| graph.import_buffer("loom.fluid", *b))
                .collect();
            let (bp, bcc, bcs, bcu, bso, bbs, bu, bv, bw, bmk, bpa, bpb, brh, bsd, bpr, bro) = (
                ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6], ids[7], ids[8], ids[9],
                ids[10], ids[11], ids[12], ids[13], ids[14], ids[17],
            );
            let faces = [bu, bv, bw];
            let rw = BufferAccess::ComputeReadWrite;
            let ro = BufferAccess::ComputeRead;

            let go = |graph: &mut RenderGraph<'_>,
                          name: &'static str,
                          pipeline: vk::Pipeline,
                          args: [u32; 4],
                          groups: u32,
                          uses: &[(BufferId, BufferAccess)]| {
                let push = Push { consts, pad: 0, args };
                graph.pass_with(name, &[], uses, move |d, cmd| {
                    record_dispatch(d, cmd, layout, pipeline, push, groups);
                });
            };

            if seed {
                // **Every device-local buffer is zeroed before the first
                // dispatch, and this is not belt and braces.** Vulkan does not
                // define the contents of fresh device memory, and the coarse
                // multigrid levels are only ever *partially* written — a level
                // cell whose children are all air is skipped by the restriction
                // — so on tick one the solve reads whatever the driver handed
                // back. It is nondeterminism at the top of a chaotic system,
                // which is the shape of bug that shows up as "three fresh
                // processes, three pictures" and nowhere else.
                //
                // Through the graph, with the accesses declared, because a
                // `vkCmdFillBuffer` outside it is a barrier written by hand
                // (never-do #4).
                let all: Vec<(BufferId, BufferAccess)> =
                    ids.iter().map(|id| (*id, rw)).collect();
                // **`WHOLE_SIZE`, not a length this code carries.** It used to
                // carry `Allocation::size()`, which is gpu-allocator's
                // *padded* size and not the buffer's: on ribbon's grid that is
                // eight bytes past the end of `loom.fluid.vel_u`, which is
                // `VUID-vkCmdFillBuffer-size-00027` in debug and an
                // out-of-bounds write in release. `WHOLE_SIZE` fills to the end
                // of the *buffer*, so the only copy of the length is the one
                // `vkCreateBuffer` was given.
                let targets: Vec<vk::Buffer> = handles
                    .iter()
                    .copied()
                    .zip(zeroed.iter().copied())
                    .filter_map(|(buffer, zero)| zero.then_some(buffer))
                    .collect();
                graph.pass_with("fluid_zero", &[], &all, move |d, cmd| {
                    for buffer in &targets {
                        // SAFETY: every buffer is live and the graph has
                        // ordered this against whatever touches them next.
                        unsafe { d.cmd_fill_buffer(cmd, *buffer, 0, vk::WHOLE_SIZE, 0) };
                    }
                });
                go(&mut graph, "fluid_seed", p.seed, [0; 4], particle_groups, &[(bp, rw)]);
            }

            // **The cascade, once per tick and before the substeps.** Once
            // rather than per substep because the rate is authored per second
            // and the substep count is an implementation detail of the solve;
            // before, so this tick's arrivals are transferred to the grid by
            // the P2G below rather than appearing after the projection with no
            // pressure ever computed for them.
            if self.inflow_per_tick > 0 {
                #[allow(clippy::cast_possible_truncation)]
                let start = (self.tick.wrapping_mul(u64::from(self.inflow_per_tick))
                    % (self.particles as u64)) as u32;
                go(&mut graph, "fluid_inflow", p.inflow, [start, self.inflow_per_tick, 0, 0],
                   particle_groups, &[(bp, rw)]);
            }

            for _ in 0..SUBSTEPS {
                go(&mut graph, "fluid_clear", p.clear_counts, [0; 4], cell_groups, &[(bcc, rw)]);
                go(&mut graph, "fluid_count", p.count, [0; 4], particle_groups,
                   &[(bp, ro), (bcc, rw)]);
                go(&mut graph, "fluid_scan_block", p.scan_block, [0; 4], cell_groups,
                   &[(bcc, ro), (bcs, rw), (bbs, rw)]);
                go(&mut graph, "fluid_scan_sums", p.scan_sums, [0; 4], 1, &[(bbs, rw)]);
                go(&mut graph, "fluid_scan_add", p.scan_add, [0; 4], cell_groups,
                   &[(bcs, rw), (bbs, ro), (bcu, rw)]);
                go(&mut graph, "fluid_scatter", p.scatter, [0; 4], particle_groups,
                   &[(bp, ro), (bcu, rw), (bso, rw)]);
                go(&mut graph, "fluid_sort", p.sort_cell, [0; 4], cell_groups,
                   &[(bcs, ro), (bcc, ro), (bso, rw)]);

                go(&mut graph, "fluid_reorder", p.reorder, [0; 4], particle_groups,
                   &[(bp, ro), (bso, ro), (bro, rw)]);

                go(&mut graph, "fluid_p2g", p.p2g, [0; 4], extended_groups,
                   &[(bro, ro), (bcs, ro), (bcc, ro), (bu, rw), (bv, rw), (bw, rw)]);
                for axis in 0..3_u32 {
                    go(&mut graph, "fluid_forces", p.forces, [0, axis, 0, 0],
                       face_groups[axis as usize],
                       &[(faces[axis as usize], rw), (bsd, ro)]);
                }

                go(&mut graph, "fluid_marker", p.marker, [0; 4], cell_groups,
                   &[(bcc, ro), (bsd, ro), (bmk, rw)]);
                for (level, groups) in level_groups.iter().enumerate().skip(1) {
                    #[allow(clippy::cast_possible_truncation)]
                    go(&mut graph, "fluid_restrict_marker", p.restrict_marker,
                       [level as u32, 0, 0, 0], *groups, &[(bmk, rw)]);
                }
                // **`bcc` is declared because the divergence reads it** — the
                // density correction in `fluidDivergenceMain` needs this
                // substep's particle count, and a dependency the graph is not
                // told about is a barrier that does not exist (never-do #4).
                // It was reaching the right answer through the marker pass's
                // chain, which is not a guarantee.
                go(&mut graph, "fluid_divergence", p.divergence, [0; 4], cell_groups,
                   &[(bcc, ro), (bu, ro), (bv, ro), (bw, ro), (bmk, ro), (brh, rw), (bpa, rw),
                     (bpb, rw)]);

                let jacobi = &[(bmk, ro), (brh, ro), (bpa, rw), (bpb, rw)];
                for _ in 0..CYCLES {
                    for level in 0..LEVELS - 1 {
                        #[allow(clippy::cast_possible_truncation)]
                        for parity in 0..SMOOTHS {
                            go(&mut graph, "fluid_jacobi", p.jacobi,
                               [level as u32, 0, parity % 2, 0], level_groups[level], jacobi);
                        }
                        #[allow(clippy::cast_possible_truncation)]
                        go(&mut graph, "fluid_restrict", p.restrict,
                           [(level + 1) as u32, 0, 0, 0], level_groups[level + 1],
                           &[(bmk, ro), (brh, rw), (bpa, rw), (bpb, rw)]);
                    }
                    for parity in 0..COARSE_SMOOTHS {
                        #[allow(clippy::cast_possible_truncation)]
                        go(&mut graph, "fluid_jacobi", p.jacobi,
                           [(LEVELS - 1) as u32, 0, parity % 2, 0], level_groups[LEVELS - 1],
                           jacobi);
                    }
                    for level in (0..LEVELS - 1).rev() {
                        #[allow(clippy::cast_possible_truncation)]
                        go(&mut graph, "fluid_prolong", p.prolong, [level as u32, 0, 0, 0],
                           level_groups[level], &[(bmk, ro), (bpa, rw)]);
                        #[allow(clippy::cast_possible_truncation)]
                        for parity in 0..SMOOTHS {
                            go(&mut graph, "fluid_jacobi", p.jacobi,
                               [level as u32, 0, parity % 2, 0], level_groups[level], jacobi);
                        }
                    }
                }

                for axis in 0..3_u32 {
                    go(&mut graph, "fluid_project", p.project, [0, axis, 0, 0],
                       face_groups[axis as usize],
                       &[(bmk, ro), (bpa, ro), (faces[axis as usize], rw)]);
                }
                go(&mut graph, "fluid_g2p", p.g2p, [0; 4], particle_groups,
                   &[(bp, rw), (bu, ro), (bv, ro), (bw, ro), (bsd, ro)]);
            }

            if probe_groups > 0 {
                // **`bcc` again, and this one has teeth.** `fluidProbeMain`
                // decides where the free surface is with
                // `cellCount[here] * 2 < FLUID_PER_CELL` — half rest density —
                // and the pass never declared that it reads the counts. It is
                // the *readback* pass, so an undeclared dependency here does
                // not stay on the GPU: it goes through buoyancy into rapier and
                // comes back next tick as the solid mask. Symptom: `slosh.loom`
                // with its ball gave three fresh processes three pictures 37%
                // apart, while the same scene with the ball deleted was
                // byte-identical.
                go(&mut graph, "fluid_probe", p.probe, [0; 4], probe_groups,
                   &[(bpr, rw), (bcc, ro), (bmk, ro), (bu, ro), (bv, ro), (bw, ro), (bpa, ro)]);
            }

            graph.execute(&device, cmd);
        }

        // SAFETY: `cmd` is recording.
        let _ = unsafe { device.end_command_buffer(cmd) };
        let info = vk::SubmitInfo::default().command_buffers(&buffers);
        // SAFETY: the fence and command buffer outlive the wait below.
        unsafe {
            let Ok(fence) = device.create_fence(&vk::FenceCreateInfo::default(), None) else {
                device.free_command_buffers(self.pool, &buffers);
                return 0.0;
            };
            let _ = device.queue_submit(submit.queue, &[info], fence);
            // Instrumentation, as above.
            #[allow(clippy::disallowed_methods)]
            let started = std::time::Instant::now();
            let _ = device.wait_for_fences(&[fence], true, u64::MAX);
            let waited = started.elapsed().as_secs_f64() * 1000.0;
            device.destroy_fence(fence, None);
            device.free_command_buffers(self.pool, &buffers);
            waited
        }
    }
}

fn record_dispatch(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    push: Push,
    groups: u32,
) {
    if groups == 0 {
        return;
    }
    // SAFETY: every handle is live for this command buffer, and the graph has
    // ordered this pass against whatever touched its buffers last.
    unsafe {
        d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::COMPUTE, 0, push.bytes());
        d.cmd_dispatch(cmd, groups, 1, 1);
    }
}

impl Drop for FluidSolver {
    fn drop(&mut self) {
        let device = self.device.handle().clone();
        // SAFETY: nothing else is using this device.
        unsafe {
            let _ = device.device_wait_idle();
            for pipeline in [
                self.pipelines.seed,
                self.pipelines.clear_counts,
                self.pipelines.count,
                self.pipelines.scan_block,
                self.pipelines.scan_sums,
                self.pipelines.scan_add,
                self.pipelines.scatter,
                self.pipelines.sort_cell,
                self.pipelines.reorder,
                self.pipelines.p2g,
                self.pipelines.forces,
                self.pipelines.marker,
                self.pipelines.restrict_marker,
                self.pipelines.divergence,
                self.pipelines.jacobi,
                self.pipelines.restrict,
                self.pipelines.prolong,
                self.pipelines.project,
                self.pipelines.g2p,
                self.pipelines.probe,
                self.pipelines.instance,
                self.pipelines.density_clear,
                self.pipelines.density_splat,
                self.pipelines.inflow,
            ] {
                device.destroy_pipeline(pipeline, None);
            }
            device.destroy_shader_module(self.module, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_command_pool(self.pool, None);

            if let Some(allocator) = self.allocator.as_mut() {
                let named = [
                    self.solid_alloc.take(),
                    self.probes_alloc.take(),
                    self.instances_alloc.take(),
                    self.consts_alloc.take(),
                    self.density_alloc.take(),
                ];
                for allocation in named.into_iter().flatten() {
                    let _ = allocator.free(allocation);
                }
                for buf in &mut self.bufs {
                    if let Some(allocation) = buf.allocation.take() {
                        let _ = allocator.free(allocation);
                    }
                    device.destroy_buffer(buf.buffer, None);
                }
            }
        }
        // The allocator must be gone before the device it borrows.
        self.allocator = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grid is 64 along the longest axis, and the cell falls out of it.
    #[test]
    fn the_longest_axis_gets_sixty_four_cells() {
        let (dims, cell) = fluid_grid([6.4, 3.2, 6.4]);
        assert_eq!(dims, [64, 32, 64]);
        assert!((cell - 0.1).abs() < 1e-6, "{cell}");
        let (dims, cell) = fluid_grid([3.2, 3.2, 3.2]);
        assert_eq!(dims, [64, 64, 64]);
        assert!((cell - 0.05).abs() < 1e-6, "{cell}");
    }

    /// The Rust and Slang halves of `FluidConsts` are one memory layout
    /// described twice, and this is the half a compiler can check.
    #[test]
    fn the_constant_block_is_the_shape_the_shader_reads() {
        // 5 vectors, 4 levels of 32 bytes, 16 device addresses.
        assert_eq!(size_of::<Level>(), 32);
        // Five vectors, four levels of 32 bytes, eighteen device addresses,
        // three more vectors for the inflow box.
        assert_eq!(size_of::<Consts>(), 80 + 128 + 18 * 8 + 48);
        assert_eq!(size_of::<Push>(), 32);
        assert_eq!(std::mem::offset_of!(Push, args), 16, "see the note on `Push::pad`");
    }

    /// Water fills the bottom half of a sealed tank and stays there.
    ///
    /// **The two failure modes this catches are opposite and both silent.** A
    /// pressure solve that does nothing lets the column collapse into a thin
    /// puddle; one whose sign is wrong blows the tank apart and every probe
    /// reads air. Between them they cover most of what can be wrong with the
    /// projection, and neither shows up as an error anywhere.
    #[test]
    fn the_tank_holds_its_water() {
        let extent = [3.2_f32, 1.6, 3.2];
        let (dims, _) = fluid_grid(extent);
        let solid = vec![0_u8; dims[0] * dims[1] * dims[2]];
        let Ok(mut solver) = FluidSolver::new(
            FluidDomain { centre: [0.0; 3], extent, fill: 0.5, inflow: None },
            &solid,
        ) else {
            // No Vulkan device here is not this test's business.
            return;
        };

        // Just under the surface, and just over it.
        let probes = [
            FluidProbe { at: [0.0, -0.2, 0.0], radius: 0.1 },
            FluidProbe { at: [0.0, 0.2, 0.0], radius: 0.1 },
        ];
        let mut out = (0.0, 0.0);
        for _ in 0..120 {
            let step = solver.step(&FluidInputs { solids: &[], probes: &probes });
            out = (step.probes[0].surface, step.probes[0].wetness);
        }
        // The tank is 1.6 m tall centred on the origin and half full, so the
        // surface sits at y = 0 give or take a cell.
        assert!(out.1 > 0.9, "the tank drained: only {} of the ring found water", out.1);
        assert!(
            out.0.abs() < 0.1,
            "the surface settled at {} rather than 0 — the tank is not holding its volume",
            out.0
        );

        // A solid ball driven down through the surface must throw water: the
        // level beside it rises. **This is the two-way half**, and it fails
        // silently — a body that displaces nothing still floats correctly,
        // because buoyancy reads the surface and the surface is flat.
        let mut peak: f32 = 0.0;
        for tick in 0_u8..40 {
            let y = 0.6 - f32::from(tick) * 0.02;
            let ball = FluidSolid {
                centre: [0.0, y, 0.0],
                half: [0.3; 3],
                radius: 0.3,
                ball: true,
                velocity: [0.0, -1.2, 0.0],
            };
            let watch = [FluidProbe { at: [0.45, 0.0, 0.0], radius: 0.08 }];
            let s = solver.step(&FluidInputs { solids: &[ball], probes: &watch });
            let v = s.probes[0].velocity;
            peak = peak.max(v[0].hypot(v[1]).hypot(v[2]));
        }
        println!("displacement: the water beside the ball reached {peak:.4} m/s");
        assert!(peak > 0.05, "a ball driven into the tank moved nothing: {peak}");

        let mut step = 0.0;
        let mut wait = 0.0;
        for _ in 0..30 {
            let out = solver.step(&FluidInputs { solids: &[], probes: &probes });
            step += out.step_ms;
            wait += out.fence_wait_ms;
        }
        println!(
            "fluid {}x{}x{} {} particles: {:.3} ms/tick, fence {:.3} ms",
            dims[0], dims[1], dims[2], solver.particle_count(), step / 30.0, wait / 30.0
        );
    }
}
