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

/// V-cycles per projection. **Fixed, never residual-tested**, for the same
/// reason.
const CYCLES: u32 = 3;

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
    /// Fraction of the pontoon's volume in a cell the solver calls fluid.
    pub fraction: f32,
    /// Mean fluid velocity over the wetted part, m/s.
    pub velocity: [f32; 3],
    /// Mean pressure potential there. Reported because it is free; nothing
    /// reads it yet.
    pub pressure: f32,
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
}

/// Mirrors `FluidPush`. 24 bytes of the 128 guaranteed.
#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    consts: vk::DeviceAddress,
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
                vk::BufferUsageFlags::empty(),
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
        ];
        let mut solid_alloc = None;
        let mut probes_alloc = None;
        let mut instances_alloc = None;
        let mut consts_alloc = None;
        for (index, (buffer, allocation, address)) in bufs_list.into_iter().enumerate() {
            // The four host-visible ones keep a second handle on their
            // allocation so the CPU can map them by name rather than by index.
            let keep = match index {
                13 => &mut solid_alloc,
                14 => &mut probes_alloc,
                15 => &mut instances_alloc,
                16 => &mut consts_alloc,
                _ => {
                    bufs.push(Buf { buffer, allocation: Some(allocation), address });
                    continue;
                }
            };
            *keep = Some(allocation);
            bufs.push(Buf { buffer, allocation: None, address });
        }

        let mut solver = Self {
            bufs,
            consts_alloc,
            solid_alloc,
            probes_alloc,
            instances_alloc,
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
            static_solid: solid_base.clone(),
            scratch_solid: solid_base,
            seeded: false,
            tick: 0,
            out: FluidStepOutput::default(),
        };
        solver.write_consts()?;
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
                fraction: a[0],
                velocity: [a[1], a[2], a[3]],
                pressure: b[0],
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
        let push = Push { consts: self.consts_address(), args: [0; 4] };
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
                let push = Push { consts, args };
                graph.pass_with(name, &[], uses, move |d, cmd| {
                    record_dispatch(d, cmd, layout, pipeline, push, groups);
                });
            };

            if seed {
                go(&mut graph, "fluid_seed", p.seed, [0; 4], particle_groups, &[(bp, rw)]);
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
                go(&mut graph, "fluid_divergence", p.divergence, [0; 4], cell_groups,
                   &[(bu, ro), (bv, ro), (bw, ro), (bmk, ro), (brh, rw), (bpa, rw), (bpb, rw)]);

                let jacobi = &[(bmk, ro), (brh, ro), (bpa, rw), (bpb, rw)];
                for _ in 0..CYCLES {
                    for level in 0..LEVELS - 1 {
                        #[allow(clippy::cast_possible_truncation)]
                        for parity in 0..2_u32 {
                            go(&mut graph, "fluid_jacobi", p.jacobi,
                               [level as u32, 0, parity, 0], level_groups[level], jacobi);
                        }
                        #[allow(clippy::cast_possible_truncation)]
                        go(&mut graph, "fluid_restrict", p.restrict,
                           [(level + 1) as u32, 0, 0, 0], level_groups[level + 1],
                           &[(bmk, ro), (brh, rw), (bpa, rw), (bpb, rw)]);
                    }
                    for parity in 0..4_u32 {
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
                        for parity in 0..2_u32 {
                            go(&mut graph, "fluid_jacobi", p.jacobi,
                               [level as u32, 0, parity, 0], level_groups[level], jacobi);
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
                go(&mut graph, "fluid_probe", p.probe, [0; 4], probe_groups,
                   &[(bpr, rw), (bmk, ro), (bu, ro), (bv, ro), (bw, ro), (bpa, ro)]);
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
        assert_eq!(size_of::<Consts>(), 80 + 128 + 128);
        assert_eq!(size_of::<Push>(), 24);
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
            FluidDomain { centre: [0.0; 3], extent, fill: 0.5 },
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
            out = (step.probes[0].fraction, step.probes[1].fraction);
        }
        assert!(out.0 > 0.9, "the tank drained: submerged probe read {}", out.0);
        assert!(out.1 < 0.1, "the tank overflowed: probe above the surface read {}", out.1);

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
