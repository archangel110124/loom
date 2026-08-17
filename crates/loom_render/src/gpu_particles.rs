//! The GPU particle pool's resources: one emitter's state, its draw instances,
//! and the compute pipeline that advances both.
//!
//! ADR 0047. `assets/shaders/gpu_particles.slang` is the other half and carries
//! the reasoning about the simulation; this file is the plumbing, and it is
//! `rain.rs` with the collision field taken out.
//!
//! # What is deliberately absent
//!
//! - **A graphics pipeline.** The pool writes `ParticleInstance`s, which is what
//!   `scene.slang`'s `particleVertexMain` already reads, so a GPU plume is drawn
//!   by the same pipeline, through the same billboard, with the same blend and
//!   the same fog as a CPU one. A second particle renderer would be a second
//!   place for the two to look different.
//! - **An indirect draw.** There is nothing to cull yet: the pool is a fixed
//!   number of slots and an empty one writes a degenerate quad. An indirect
//!   draw arrives with a cull, via a one-thread args pass à la
//!   `rainSplashArgsMain`, and not before.
//! - **Every barrier.** They belong to the render graph (never-do #4), which
//!   already grew [`loom_render_graph::BufferAccess`] for rain: a compute pass
//!   writes the instance buffer and a vertex shader reads it in the same
//!   command buffer, and without a dependency the draw shows last frame's
//!   particles — which looks almost right.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};
use gpu_allocator::MemoryLocation;

use crate::debug_names::DebugNames;
use crate::renderer::{
    create_address_buffer_in, create_shader_module, RenderError, write_slice,
};

/// Slots one pool holds.
///
/// **Mirrors `loom_scene::components::GPU_POOL_MAX`**, which is the
/// authoritative copy because it is an authoring constraint first — the load
/// validator has to put the number in its rejection and `loom_scene` depends on
/// nothing that could tell it. A test below reads that file and compares, the
/// same way `rain.rs` reads `include/rain.slang` for `RAIN_DROPS`.
pub const POOL: u32 = 65_536;

/// Compute threads per workgroup, mirroring `[numthreads(64,1,1)]`.
const GROUP: u32 = 64;

/// One pooled particle, mirroring `GpuParticle` in the shader.
///
/// Never written by the CPU — the dispatch seeds it. Declared here only so the
/// buffer can be sized from something the compiler checks.
#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    position: [f32; 4],
    velocity: [f32; 4],
}

/// One emitter, as the pool simulates it.
///
/// **The CPU's whole contribution to a GPU plume.** Everything else — where a
/// particle is, how old it is, whether it exists — is the device's, and none of
/// it comes back (ADR 0045 clause 2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuEmitter {
    /// World position particles are born at.
    pub origin: [f32; 3],
    /// Radius of the disc they are born on.
    pub radius: f32,
    pub speed: f32,
    /// Half-angle of the emission cone about +Y, in **radians**. Converted on
    /// this side so the shader never carries a degree-to-radian constant that
    /// could drift from the component's.
    pub spread: f32,
    pub gravity: f32,
    pub drag: f32,
    pub turbulence: f32,
    pub turbulence_scale: f32,
    pub wind_response: f32,
    pub lifetime: f32,
    pub lifetime_jitter: f32,
    pub rate: f32,
    pub delay: f32,
    pub duration: f32,
    pub burst: u32,
    /// Diameter at birth and at death.
    pub size: [f32; 2],
    pub color_start: [f32; 3],
    pub color_end: [f32; 3],
    /// Opacity at birth and at death.
    pub alpha: [f32; 2],
    pub additive: bool,
    pub flame: bool,
    pub seed: u32,
    /// Slots this emitter needs. `loom_particles::pool_size`, computed by the
    /// caller and clamped here — the load validator has already refused
    /// anything over [`POOL`], so a clamp reaching is a bug rather than a
    /// policy.
    pub pool: u32,
}

/// The per-emitter parameter block, mirroring `GpuEmitterParams`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Params {
    origin: [f32; 4],
    motion: [f32; 4],
    swirl: [f32; 4],
    life: [f32; 4],
    size_alpha: [f32; 4],
    color_start: [f32; 4],
    color_end: [f32; 4],
    wind: [f32; 4],
    weather: [f32; 4],
}

/// The push block, mirroring `GpuParticlePush`.
///
/// **72 bytes of the 128 Vulkan guarantees.** Vectors first, then the pointers,
/// which need 8-byte alignment; every member lands on its natural one with no
/// padding, and a test pins the offsets against the compiled module.
#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    /// x the fixed timestep in seconds.
    clock: [f32; 4],
    /// x pool slots, y ticks to advance, z 1 to clear, w tick at the start.
    control: [u32; 4],
    /// x emitter seed, y burst, z additive, w flame.
    ident: [u32; 4],
    params: vk::DeviceAddress,
    pool: vk::DeviceAddress,
    instances: vk::DeviceAddress,
}

impl Push {
    fn bytes(&self) -> &[u8] {
        // SAFETY: `Push` is `repr(C)` and plain old data.
        unsafe {
            std::slice::from_raw_parts(std::ptr::from_ref(self).cast::<u8>(), size_of::<Self>())
        }
    }
}

/// Everything the GPU particle layer owns on the device.
pub(crate) struct GpuParticles {
    device: ash::Device,

    /// Per-slot state. Device-local: nothing on the CPU ever reads or writes a
    /// particle, and this buffer is touched by every dispatch.
    pool: vk::Buffer,
    pool_alloc: Option<Allocation>,
    pool_address: vk::DeviceAddress,

    /// What the draw reads. Device-local for the same reason, and it is the
    /// only thing the graphics pipeline sees.
    instances: vk::Buffer,
    instances_alloc: Option<Allocation>,
    pub(crate) instances_address: vk::DeviceAddress,

    /// The emitter's parameters. Host-visible and rewritten every frame — it is
    /// 144 bytes, and the alternative is a staging copy for less than a cache
    /// line's worth of scalars.
    params: vk::Buffer,
    params_alloc: Option<Allocation>,
    params_address: vk::DeviceAddress,

    layout: vk::PipelineLayout,
    module: vk::ShaderModule,
    simulate: vk::Pipeline,

    /// The emitter this pool is currently simulating, and the tick its state
    /// stands for.
    ///
    /// **This pair is the whole of the history-dependence.** A fresh renderer,
    /// or one whose emitter changed, seeds and then advances by the difference,
    /// so a headless still at `--sim N` is always seed-then-advance-N — which
    /// is the property `cargo xtask repeat` checks rather than assumes.
    emitter: Option<GpuEmitter>,
    tick: Option<u64>,
}

impl GpuParticles {
    pub(crate) fn new(
        instance: &ash::Instance,
        device: &crate::Device,
        allocator: &mut Allocator,
        cache: vk::PipelineCache,
    ) -> Result<Self, RenderError> {
        let raw = device.handle().clone();
        let names = DebugNames::new(instance, &raw, cfg!(debug_assertions));

        let (pool, pool_alloc, pool_address) = create_address_buffer_in(
            &raw,
            allocator,
            u64::from(POOL) * size_of::<Slot>() as u64,
            "loom.gpu_particles.pool",
            vk::BufferUsageFlags::empty(),
            MemoryLocation::GpuOnly,
        )?;
        let (instances, instances_alloc, instances_address) = create_address_buffer_in(
            &raw,
            allocator,
            u64::from(POOL) * size_of::<crate::renderer::ParticleInstance>() as u64,
            "loom.gpu_particles.instances",
            vk::BufferUsageFlags::empty(),
            MemoryLocation::GpuOnly,
        )?;
        let (params, params_alloc, params_address) = create_address_buffer_in(
            &raw,
            allocator,
            size_of::<Params>() as u64,
            "loom.gpu_particles.params",
            vk::BufferUsageFlags::empty(),
            MemoryLocation::CpuToGpu,
        )?;
        write_slice(&params_alloc, &[Params::default()])?;
        names.set(pool, "loom.gpu_particles.pool");
        names.set(instances, "loom.gpu_particles.instances");
        names.set(params, "loom.gpu_particles.params");

        let range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(u32::try_from(size_of::<Push>()).unwrap_or(128));
        let ranges = [range];
        let layout_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&ranges);
        // SAFETY: `ranges` outlives the call, and this layout binds no
        // descriptor sets — everything it reads is a device address.
        let layout = unsafe { raw.create_pipeline_layout(&layout_info, None) }?;
        names.set(layout, "loom.gpu_particles.layout");

        let module = create_shader_module(&raw, crate::GPU_PARTICLES_SPV)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(c"gpuParticleSimulateMain");
        let info = vk::ComputePipelineCreateInfo::default().stage(stage).layout(layout);
        // SAFETY: `info` borrows `module` and `layout`, both of which outlive it.
        let simulate = match unsafe { raw.create_compute_pipelines(cache, &[info], None) } {
            Ok(pipelines) => pipelines[0],
            Err((_, e)) => return Err(RenderError::Vulkan(e)),
        };
        names.set(simulate, "loom.gpu_particles.simulate");

        Ok(Self {
            device: raw,
            pool,
            pool_alloc: Some(pool_alloc),
            pool_address,
            instances,
            instances_alloc: Some(instances_alloc),
            instances_address,
            params,
            params_alloc: Some(params_alloc),
            params_address,
            layout,
            module,
            simulate,
            emitter: None,
            tick: None,
        })
    }

    /// Point the pool at an emitter, or at none.
    ///
    /// **Changing the emitter re-seeds**, because the particles in flight
    /// belong to a plume that no longer exists — carrying them over would show
    /// the old emitter's spray drifting under the new one's parameters.
    pub(crate) fn set_emitter(&mut self, emitter: Option<GpuEmitter>) {
        if self.emitter != emitter {
            self.emitter = emitter;
            self.tick = None;
        }
    }

    /// Slots the draw should expand into quads, or zero when there is nothing.
    pub(crate) fn count(&self) -> u32 {
        self.emitter.map_or(0, |e| e.pool.min(POOL))
    }

    /// Record this frame's dispatch into `graph`, returning the instance buffer
    /// so the caller can declare its vertex read.
    ///
    /// `wind` and `weather` are the environment block's own four-vectors, so a
    /// GPU plume sits in exactly the air grass does.
    pub(crate) fn record(
        &mut self,
        graph: &mut loom_render_graph::RenderGraph<'_>,
        to_tick: u64,
        dt: f32,
        wind: [f32; 4],
        weather: [f32; 4],
    ) -> Option<loom_render_graph::BufferId> {
        use loom_render_graph::BufferAccess;

        let emitter = self.emitter?;
        // **Going backwards re-seeds**, because the recurrence cannot be run in
        // reverse. A viewer that restarts a scene therefore needs no reset
        // call; `Renderer::set_rain_tick` makes the same promise.
        let seed = self.tick.is_none_or(|last| to_tick < last);
        let steps =
            u32::try_from(to_tick.saturating_sub(if seed { 0 } else { self.tick.unwrap_or(0) }))
                .unwrap_or(u32::MAX);
        self.tick = Some(to_tick);

        let params = Params {
            origin: [emitter.origin[0], emitter.origin[1], emitter.origin[2], emitter.radius],
            motion: [emitter.speed, emitter.spread, emitter.gravity, emitter.drag],
            swirl: [
                emitter.turbulence,
                emitter.turbulence_scale,
                emitter.wind_response,
                emitter.lifetime,
            ],
            life: [emitter.lifetime_jitter, emitter.rate, emitter.delay, emitter.duration],
            size_alpha: [emitter.size[0], emitter.size[1], emitter.alpha[0], emitter.alpha[1]],
            color_start: [
                emitter.color_start[0],
                emitter.color_start[1],
                emitter.color_start[2],
                0.0,
            ],
            color_end: [emitter.color_end[0], emitter.color_end[1], emitter.color_end[2], 0.0],
            wind,
            weather,
        };
        // A failed write leaves last frame's parameters, which is wrong but not
        // unsafe; the buffer is 144 bytes of host-visible memory and this
        // cannot fail in practice. Swallowing it keeps `record` infallible,
        // which is what lets it sit inside the graph build.
        if let Some(alloc) = self.params_alloc.as_ref() {
            let _ = write_slice(alloc, &[params]);
        }

        let count = emitter.pool.min(POOL);
        let push = Push {
            clock: [dt, 0.0, 0.0, 0.0],
            control: [
                count,
                steps,
                u32::from(seed),
                u32::try_from(to_tick.saturating_sub(u64::from(steps))).unwrap_or(0),
            ],
            ident: [
                emitter.seed,
                emitter.burst,
                u32::from(emitter.additive),
                u32::from(emitter.flame),
            ],
            params: self.params_address,
            pool: self.pool_address,
            instances: self.instances_address,
        };

        let pool_id = graph.import_buffer("loom.gpu_particles.pool", self.pool);
        let instances_id = graph.import_buffer("loom.gpu_particles.instances", self.instances);

        let (layout, simulate) = (self.layout, self.simulate);
        graph.pass_with(
            "gpu_particles",
            &[],
            &[
                (pool_id, BufferAccess::ComputeReadWrite),
                (instances_id, BufferAccess::ComputeReadWrite),
            ],
            move |d, cmd| {
                // SAFETY: every handle is live for this command buffer, and the
                // graph has ordered this against whatever touched the buffers
                // last.
                unsafe {
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, simulate);
                    d.cmd_push_constants(
                        cmd,
                        layout,
                        vk::ShaderStageFlags::COMPUTE,
                        0,
                        push.bytes(),
                    );
                    d.cmd_dispatch(cmd, count.div_ceil(GROUP), 1, 1);
                }
            },
        );

        Some(instances_id)
    }

    /// # Safety
    /// Nothing may still be in flight against these.
    pub(crate) unsafe fn destroy(&mut self, allocator: &mut Allocator) {
        // SAFETY: the caller guarantees nothing is in flight.
        unsafe {
            self.device.destroy_pipeline(self.simulate, None);
            self.device.destroy_shader_module(self.module, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device.destroy_buffer(self.pool, None);
            self.device.destroy_buffer(self.instances, None);
            self.device.destroy_buffer(self.params, None);
        }
        for allocation in [
            self.pool_alloc.take(),
            self.instances_alloc.take(),
            self.params_alloc.take(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = allocator.free(allocation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The push block is one memory layout described twice**, and the second
    /// description is in a shader the Rust compiler never sees. Offsets read
    /// out of the compiled module with
    /// `spirv-dis | grep 'OpMemberDecorate %GpuParticlePush'`.
    #[test]
    fn the_push_block_is_laid_out_as_the_shader_reads_it() {
        let push = Push {
            clock: [0.0; 4],
            control: [0; 4],
            ident: [0; 4],
            params: 0,
            pool: 0,
            instances: 0,
        };
        let at = |field: *const u8| field as usize - std::ptr::from_ref(&push).cast::<u8>() as usize;

        assert_eq!(at(std::ptr::from_ref(&push.control).cast()), 16, "control");
        assert_eq!(at(std::ptr::from_ref(&push.ident).cast()), 32, "ident");
        assert_eq!(at(std::ptr::from_ref(&push.params).cast()), 48, "params");
        assert_eq!(at(std::ptr::from_ref(&push.pool).cast()), 56, "pool");
        assert_eq!(at(std::ptr::from_ref(&push.instances).cast()), 64, "instances");
        assert_eq!(size_of::<Push>(), 72, "the whole block");
        assert!(size_of::<Push>() <= 128, "past the guaranteed push-constant size");
    }

    /// The pool ceiling is authored in `loom_scene`, because the load validator
    /// has to put it in a rejection and that crate may depend on nothing that
    /// could tell it. Two declarations of one number, so something has to
    /// compare them — this is the same trick `rain.rs` plays on the shader
    /// header.
    #[test]
    fn the_pool_ceiling_matches_the_one_the_validator_refuses_against() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../loom_scene/src/components.rs"
        ))
        .expect("loom_scene is a sibling crate");

        let needle = "pub const GPU_POOL_MAX: u32 = ";
        let start = source.find(needle).expect("GPU_POOL_MAX") + needle.len();
        let rest = &source[start..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '_')
            .expect("a number");
        let declared: u32 = rest[..end].replace('_', "").parse().expect("a number");

        assert_eq!(declared, POOL);
    }

    /// The two buffers the pool owns are sized in slots, and the sizes have to
    /// be the shader's — an undersized pool is a device-address write past the
    /// end, which is a hang rather than a validation message.
    #[test]
    fn the_slot_and_instance_records_are_two_float4s_each() {
        assert_eq!(size_of::<Slot>(), 32, "GpuParticle is two float4s");
        assert_eq!(
            size_of::<crate::renderer::ParticleInstance>(),
            32,
            "the draw's instance is the CPU path's, unchanged"
        );
    }
}
