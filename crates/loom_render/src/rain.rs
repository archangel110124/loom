//! The raindrop simulation's GPU resources: a drop buffer, the world it
//! collides with, and the two compute pipelines that advance it.
//!
//! ADR 0015. `rain_sim.slang` is the other half and carries the reasoning about
//! the simulation itself; this file is the plumbing.
//!
//! # Barriers
//!
//! All of them belong to the render graph (never-do #4), which grew
//! [`loom_render_graph::BufferAccess`] for exactly this: a compute pass writes
//! the drop buffer and a vertex shader reads it in the same command buffer, and
//! without a dependency between them the draw reads last frame's drops. That is
//! not a corruption you can see — the picture is *almost* right — which is the
//! worst kind.
//!
//! The one exception is the collision field's upload, which is a one-shot
//! staging copy outside any frame, following the precedent `material.rs` sets
//! and documents.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};
use gpu_allocator::MemoryLocation;

use crate::debug_names::DebugNames;
use crate::material::record;
use crate::raytrace::Submit;
use crate::renderer::{
    RenderError, create_address_buffer_in, create_image_3d, create_shader_module, write_slice,
};

/// Drops the simulation carries. **Mirrors `RAIN_DROPS` in `include/rain.slang`
/// exactly** — the shader sizes its bounds check by its copy and this sizes the
/// buffer, and a mismatch is a device-address read past the end.
pub const DROPS: u32 = 131_072;

/// Entries in the splash ring. Mirrors `RAIN_SPLASH_RING`.
pub const SPLASH_RING: u32 = 16_384;

/// Compute threads per workgroup, mirroring `[numthreads(64,1,1)]`.
const GROUP: u32 = 64;

/// Trailing ticks of a catch-up run that may append a splash.
///
/// **Thirty, which is `RAIN_SPLASH_RING` divided by a tick's landings** — the
/// whole ring, and no more. Emitting on every tick of an 1,800-tick catch-up
/// would lap the ring sixty times for a picture identical to emitting on the
/// last thirty, and would make the surviving set depend on atomic ordering.
const SPLASH_STEPS: u32 = 30;

/// One drop, mirroring `RainDrop` in `include/rain.slang`.
///
/// Never written by the CPU — the shader seeds it. Declared here only so the
/// buffer can be sized from something the compiler checks.
#[repr(C)]
#[derive(Clone, Copy)]
struct Drop {
    position: [f32; 4],
    velocity: [f32; 4],
}

/// One splash, mirroring `RainSplash`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Splash {
    impact: [f32; 4],
    surface: [f32; 4],
}

/// The push block, mirroring `RainSimPush` in `rain_sim.slang`.
///
/// **120 bytes of the 128 Vulkan guarantees.** Vectors first, then the two-word
/// control, then the pointers: a device address needs 8-byte alignment, and
/// every member here lands on its natural one with no padding. The offsets were
/// read out of the compiled module with
/// `spirv-dis | grep 'OpMemberDecorate %RainSimPush'` and are pinned by a test.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Push {
    /// xyz eye, w simulated seconds at the end of the run.
    pub eye: [f32; 4],
    /// xyz wind m/s, w unsheltered rate in mm/h.
    pub wind: [f32; 4],
    /// xyz world centre of collision voxel (0,0,0), w metres per voxel.
    pub field: [f32; 4],
    /// xy terrain grid origin, z spacing, w samples per axis.
    pub terrain: [f32; 4],
    /// xyz collision voxels per axis, w ticks to advance.
    pub dims: [u32; 4],
    /// x 1 to seed, y trailing ticks that may splash.
    pub control: [u32; 2],
    pub drops: vk::DeviceAddress,
    pub splashes: vk::DeviceAddress,
    pub splash_args: vk::DeviceAddress,
    pub terrain_heights: vk::DeviceAddress,
}

impl Push {
    fn bytes(&self) -> &[u8] {
        // SAFETY: `Push` is `repr(C)` and plain old data.
        unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(self).cast::<u8>(),
                size_of::<Self>(),
            )
        }
    }
}

/// The world a drop collides with, as an `R8_SNORM` 3D image.
struct Field {
    image: vk::Image,
    view: vk::ImageView,
    allocation: Option<Allocation>,
    /// World centre of voxel (0,0,0), and metres per voxel.
    origin: [f32; 3],
    spacing: f32,
    dims: [u32; 3],
}

/// Everything the rain layer owns on the GPU.
pub(crate) struct RainSim {
    device: ash::Device,
    /// Kept so a re-bake can name its image. Naming is what makes a validation
    /// message about the collision field say so (brief 7.3).
    names: DebugNames,

    drops: vk::Buffer,
    drops_alloc: Option<Allocation>,
    pub(crate) drops_address: vk::DeviceAddress,

    splashes: vk::Buffer,
    splashes_alloc: Option<Allocation>,
    pub(crate) splashes_address: vk::DeviceAddress,

    /// `VkDrawIndirectCommand` in words 0..3, the ring cursor in word 4.
    args: vk::Buffer,
    args_alloc: Option<Allocation>,
    args_address: vk::DeviceAddress,

    field: Field,
    sampler: vk::Sampler,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,

    layout: vk::PipelineLayout,
    module: vk::ShaderModule,
    simulate: vk::Pipeline,
    splash_args: vk::Pipeline,

    /// The tick this buffer's state stands for. `None` until it is seeded.
    ///
    /// **This is the whole of the history-dependence**, and having it in one
    /// `Option<u64>` is what makes the reproducibility argument checkable: a
    /// fresh renderer seeds and then advances by the difference, so a headless
    /// still at `--sim N` is always seed-then-advance-N.
    tick: Option<u64>,
}

impl RainSim {
    pub(crate) fn new(
        instance: &ash::Instance,
        device: &crate::Device,
        allocator: &mut Allocator,
        cache: vk::PipelineCache,
        submit: Submit,
    ) -> Result<Self, RenderError> {
        let raw = device.handle().clone();
        let names = DebugNames::new(instance, &raw, cfg!(debug_assertions));
        let names = &names;

        // **Device-local, not host-visible.** Nothing on the CPU ever writes a
        // drop — the shader seeds them — and the buffer is read and written by
        // every dispatch, which over a host-visible heap would be a PCIe round
        // trip per frame.
        let (drops, drops_alloc, drops_address) = create_address_buffer_in(
            &raw,
            allocator,
            u64::from(DROPS) * size_of::<Drop>() as u64,
            "loom.rain.drops",
            vk::BufferUsageFlags::empty(),
            MemoryLocation::GpuOnly,
        )?;
        let (splashes, splashes_alloc, splashes_address) = create_address_buffer_in(
            &raw,
            allocator,
            u64::from(SPLASH_RING) * size_of::<Splash>() as u64,
            "loom.rain.splashes",
            vk::BufferUsageFlags::empty(),
            MemoryLocation::GpuOnly,
        )?;
        // Host-visible, because it starts at zero and the CPU is the only thing
        // that can put it there before the first dispatch. Twenty bytes.
        let (args, args_alloc, args_address) = create_address_buffer_in(
            &raw,
            allocator,
            32,
            "loom.rain.splash_args",
            vk::BufferUsageFlags::INDIRECT_BUFFER,
            MemoryLocation::CpuToGpu,
        )?;
        write_slice(&args_alloc, &[0_u32; 8])?;
        names.set(drops, "loom.rain.drops");
        names.set(splashes, "loom.rain.splashes");
        names.set(args, "loom.rain.splash_args");

        // A one-voxel field of open air, so the descriptor is always valid even
        // in a scene with nothing to collide with. Replaced wholesale by
        // `set_field`.
        //
        // **It gets its layout transition here, and skipping that was a
        // validation error rather than a saving.** The first version deferred
        // it to the first real upload, on the theory that nothing samples the
        // placeholder — which is false twice over: a rain scene with no voxels
        // and no colliders never gets an upload at all, and in the windowed
        // path the first frames draw before the scene's bake has landed. The
        // layers said so, ten times, on the first windowed run.
        let field = Field::empty(&raw, allocator, names, submit)?;

        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            // **Clamped, and the choice matters.** A drop that steps outside
            // the field must read the edge voxel rather than wrap to the far
            // side of the world, and the shader's own bounds test hands it to
            // the height field before that can happen anyway.
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: nothing is borrowed by the info.
        let sampler = unsafe { raw.create_sampler(&sampler_info, None) }?;
        names.set(sampler, "loom.rain.field_sampler");

        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE);
        let bindings = [binding];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: `bindings` outlives the call.
        let set_layout = unsafe { raw.create_descriptor_set_layout(&layout_info, None) }?;

        let size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1);
        let sizes = [size];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(&sizes)
            .max_sets(1);
        // SAFETY: `sizes` outlives the call.
        let pool = unsafe { raw.create_descriptor_pool(&pool_info, None) }?;
        let layouts = [set_layout];
        let allocate = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: `layouts` outlives the call and the pool holds one set.
        let set = unsafe { raw.allocate_descriptor_sets(&allocate) }?[0];

        let range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(u32::try_from(size_of::<Push>()).unwrap_or(128));
        let ranges = [range];
        let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&ranges);
        // SAFETY: both slices outlive the call.
        let layout = unsafe { raw.create_pipeline_layout(&pipeline_layout_info, None) }?;
        names.set(layout, "loom.rain.sim_layout");

        let module = create_shader_module(&raw, crate::RAIN_SIM_SPV)?;
        let simulate = compute_pipeline(&raw, cache, layout, module, c"rainSimulateMain")?;
        let splash_args = compute_pipeline(&raw, cache, layout, module, c"rainSplashArgsMain")?;
        names.set(simulate, "loom.rain.simulate");
        names.set(splash_args, "loom.rain.splash_args");

        let sim = Self {
            device: raw,
            names: DebugNames::new(instance, device.handle(), cfg!(debug_assertions)),
            drops,
            drops_alloc: Some(drops_alloc),
            drops_address,
            splashes,
            splashes_alloc: Some(splashes_alloc),
            splashes_address,
            args,
            args_alloc: Some(args_alloc),
            args_address,
            field,
            sampler,
            set_layout,
            pool,
            set,
            layout,
            module,
            simulate,
            splash_args,
            tick: None,
        };
        sim.bind_field();
        Ok(sim)
    }

    /// Point the descriptor at the current collision field.
    fn bind_field(&self) {
        let info = vk::DescriptorImageInfo::default()
            .sampler(self.sampler)
            .image_view(self.field.view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let infos = [info];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .dst_array_element(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&infos);
        // SAFETY: the view is live and `infos` outlives the call.
        unsafe { self.device.update_descriptor_sets(&[write], &[]) };
    }

    /// Replace the collision field.
    ///
    /// **Re-uploaded when the world changes and at no other time**, which is
    /// what ADR 0014 asked for: a 2.4 MB copy on a terrain edit rather than per
    /// frame. Seeding is reset with it, because a drop resting on a wall that
    /// has just been carved away has nowhere sensible to be.
    pub(crate) fn set_field(
        &mut self,
        allocator: &mut Allocator,
        submit: Submit,
        sdf: &[i8],
        dims: [usize; 3],
        origin: [f32; 3],
        spacing: f32,
    ) -> Result<(), RenderError> {
        let field = Field::upload(
            &self.device,
            allocator,
            &self.names,
            submit,
            sdf,
            dims,
            origin,
            spacing,
        )?;
        // SAFETY: the caller has waited for the device to be idle before
        // changing the world, and `upload` waited on its own submit.
        unsafe { self.field.destroy(&self.device, allocator) };
        self.field = field;
        self.bind_field();
        self.tick = None;
        Ok(())
    }

    /// Whether a scene-sized collision field has been uploaded.
    pub(crate) fn has_field(&self) -> bool {
        self.field.dims[0] > 1
    }

    /// Forget the seeding, so the next advance starts from tick zero.
    pub(crate) fn reset(&mut self) {
        self.tick = None;
    }

    /// Record the frame's rain work into `graph`.
    ///
    /// `to_tick` is the simulation tick this frame stands for. The dispatch
    /// advances by the difference since the last call, which is one tick in a
    /// live viewer and `--sim N` in a fresh headless render — **one dispatch
    /// either way**, because thread `i` only ever touches drop `i` and the
    /// recurrence rolls up in registers.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &mut self,
        graph: &mut loom_render_graph::RenderGraph<'_>,
        to_tick: u64,
        eye: [f32; 3],
        wind: [f32; 4],
        terrain: [f32; 4],
        terrain_heights: vk::DeviceAddress,
    ) -> (loom_render_graph::BufferId, loom_render_graph::BufferId) {
        use loom_render_graph::BufferAccess;

        let seed = self.tick.is_none();
        let steps = u32::try_from(to_tick.saturating_sub(self.tick.unwrap_or(0))).unwrap_or(u32::MAX);
        // A frame that has not advanced still has to run once when seeding:
        // an unseeded buffer is whatever the allocator left there.
        let steps = if seed { steps.max(1) } else { steps };
        self.tick = Some(to_tick);

        #[allow(clippy::cast_precision_loss)]
        let seconds = to_tick as f32 / 60.0;
        let push = Push {
            eye: [eye[0], eye[1], eye[2], seconds],
            wind,
            field: [
                self.field.origin[0],
                self.field.origin[1],
                self.field.origin[2],
                self.field.spacing,
            ],
            terrain,
            dims: [self.field.dims[0], self.field.dims[1], self.field.dims[2], steps],
            control: [u32::from(seed), SPLASH_STEPS],
            drops: self.drops_address,
            splashes: self.splashes_address,
            splash_args: self.args_address,
            terrain_heights,
        };

        let drops_id = graph.import_buffer("loom.rain.drops", self.drops);
        let splashes_id = graph.import_buffer("loom.rain.splashes", self.splashes);
        let args_id = graph.import_buffer("loom.rain.splash_args", self.args);

        let (layout, set, simulate, splash_args) =
            (self.layout, self.set, self.simulate, self.splash_args);
        graph.pass_with(
            "rain_simulate",
            &[],
            &[
                (drops_id, BufferAccess::ComputeReadWrite),
                (splashes_id, BufferAccess::ComputeReadWrite),
                (args_id, BufferAccess::ComputeReadWrite),
            ],
            move |d, cmd| {
                // SAFETY: every handle is live for this command buffer, and the
                // graph has ordered this against whatever touched the buffers
                // last.
                unsafe {
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, simulate);
                    d.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::COMPUTE,
                        layout,
                        0,
                        &[set],
                        &[],
                    );
                    d.cmd_push_constants(
                        cmd,
                        layout,
                        vk::ShaderStageFlags::COMPUTE,
                        0,
                        push.bytes(),
                    );
                    d.cmd_dispatch(cmd, DROPS.div_ceil(GROUP), 1, 1);
                }
            },
        );

        graph.pass_with(
            "rain_splash_args",
            &[],
            &[(args_id, BufferAccess::ComputeReadWrite)],
            move |d, cmd| {
                // SAFETY: as above. The graph has put a barrier between this
                // and the dispatch that incremented the cursor, which is the
                // entire reason it is a second pass.
                unsafe {
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, splash_args);
                    d.cmd_push_constants(
                        cmd,
                        layout,
                        vk::ShaderStageFlags::COMPUTE,
                        0,
                        push.bytes(),
                    );
                    d.cmd_dispatch(cmd, 1, 1, 1);
                }
            },
        );

        (drops_id, args_id)
    }

    /// The indirect buffer the splash draw reads its vertex count from.
    pub(crate) fn args_buffer(&self) -> vk::Buffer {
        self.args
    }

    /// # Safety
    /// Nothing may still be in flight against these.
    pub(crate) unsafe fn destroy(&mut self, allocator: &mut Allocator) {
        // SAFETY: the caller guarantees nothing is in flight.
        unsafe {
            self.device.destroy_pipeline(self.simulate, None);
            self.device.destroy_pipeline(self.splash_args, None);
            self.device.destroy_shader_module(self.module, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.set_layout, None);
            self.device.destroy_sampler(self.sampler, None);
            self.field.destroy(&self.device, allocator);
            self.device.destroy_buffer(self.drops, None);
            self.device.destroy_buffer(self.splashes, None);
            self.device.destroy_buffer(self.args, None);
        }
        for allocation in [
            self.drops_alloc.take(),
            self.splashes_alloc.take(),
            self.args_alloc.take(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = allocator.free(allocation);
        }
    }
}

impl Field {
    /// A single voxel of open air — the descriptor's resting state.
    fn empty(
        device: &ash::Device,
        allocator: &mut Allocator,
        names: &DebugNames,
        submit: Submit,
    ) -> Result<Self, RenderError> {
        // 127 is the `i8` saturation, which the shader reads as one full
        // `range()` of clear air. Every drop then falls through to the terrain
        // height grid, which is exactly right for a scene with no solids.
        Self::upload(device, allocator, names, submit, &[127], [1, 1, 1], [0.0; 3], 1.0)
    }

    #[allow(clippy::too_many_arguments)]
    fn upload(
        device: &ash::Device,
        allocator: &mut Allocator,
        names: &DebugNames,
        submit: Submit,
        sdf: &[i8],
        dims: [usize; 3],
        origin: [f32; 3],
        spacing: f32,
    ) -> Result<Self, RenderError> {
        let extent = vk::Extent3D {
            width: u32::try_from(dims[0]).unwrap_or(1).max(1),
            height: u32::try_from(dims[1]).unwrap_or(1).max(1),
            depth: u32::try_from(dims[2]).unwrap_or(1).max(1),
        };
        let (image, allocation) = create_image_3d(
            device,
            allocator,
            extent,
            // `R8_SNORM` is a mandatory sampled + linear-filterable format, so
            // this needs no capability query. One byte per voxel is what makes
            // ADR 0014's 2.2 MB estimate hold.
            vk::Format::R8_SNORM,
            vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            "loom.rain.collision",
        )?;
        names.set(image, "loom.rain.collision");

        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(1)
            .base_array_layer(0)
            .layer_count(1);

        // Staged even for the one-voxel case, because the layout still has to
        // move out of UNDEFINED before anything samples it.
        let (staging, staging_alloc, _) = create_address_buffer_in(
            device,
            allocator,
            (sdf.len() as u64).max(4),
            "loom.rain.collision_staging",
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
        )?;
        write_slice(&staging_alloc, sdf)?;

        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_extent(extent);

        let copy = |cmd: vk::CommandBuffer| {
            let to_dst = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
                .src_access_mask(vk::AccessFlags2::empty())
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(image)
                .subresource_range(range);
            let barriers = [to_dst];
            let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
            // SAFETY: `cmd` is recording and `image` is live.
            unsafe { device.cmd_pipeline_barrier2(cmd, &dependency) };
            // SAFETY: the region covers exactly the bytes written to staging.
            unsafe {
                device.cmd_copy_buffer_to_image(
                    cmd,
                    staging,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
            let to_read = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(image)
                .subresource_range(range);
            let barriers = [to_read];
            let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
            // SAFETY: `cmd` is recording and `image` is live.
            unsafe { device.cmd_pipeline_barrier2(cmd, &dependency) };
        };

        let result = record(device, submit, copy);

        // SAFETY: the submit above has been waited on.
        unsafe { device.destroy_buffer(staging, None) };
        let _ = allocator.free(staging_alloc);
        result?;

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_3D)
            .format(vk::Format::R8_SNORM)
            .subresource_range(range);
        // SAFETY: `image` is live and the range is within it.
        let view = unsafe { device.create_image_view(&view_info, None) }?;

        Ok(Self {
            image,
            view,
            allocation: Some(allocation),
            origin,
            spacing,
            dims: [extent.width, extent.height, extent.depth],
        })
    }

    /// # Safety
    /// Nothing may still be in flight against this image.
    unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        // SAFETY: the caller guarantees nothing is in flight.
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(allocation) = self.allocation.take() {
            let _ = allocator.free(allocation);
        }
    }
}

fn compute_pipeline(
    device: &ash::Device,
    cache: vk::PipelineCache,
    layout: vk::PipelineLayout,
    module: vk::ShaderModule,
    entry: &std::ffi::CStr,
) -> Result<vk::Pipeline, RenderError> {
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(entry);
    let info = vk::ComputePipelineCreateInfo::default().stage(stage).layout(layout);
    // SAFETY: `info` borrows `module` and `layout`, both of which outlive it.
    match unsafe { device.create_compute_pipelines(cache, &[info], None) } {
        Ok(pipelines) => Ok(pipelines[0]),
        Err((_, e)) => Err(RenderError::Vulkan(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The push block is one memory layout described twice**, and the second
    /// description is in a shader the compiler never sees. The offsets are
    /// Slang's, read out of the compiled module with
    /// `spirv-dis | grep 'OpMemberDecorate %RainSimPush'`.
    #[test]
    fn the_rain_push_block_is_laid_out_as_the_shader_reads_it() {
        let push = Push {
            eye: [0.0; 4],
            wind: [0.0; 4],
            field: [0.0; 4],
            terrain: [0.0; 4],
            dims: [0; 4],
            control: [0; 2],
            drops: 0,
            splashes: 0,
            splash_args: 0,
            terrain_heights: 0,
        };
        let at = |field: *const u8| field as usize - std::ptr::from_ref(&push).cast::<u8>() as usize;

        assert_eq!(at(std::ptr::from_ref(&push.dims).cast()), 64, "dims");
        assert_eq!(at(std::ptr::from_ref(&push.control).cast()), 80, "control");
        assert_eq!(at(std::ptr::from_ref(&push.drops).cast()), 88, "drops");
        assert_eq!(at(std::ptr::from_ref(&push.splashes).cast()), 96, "splashes");
        assert_eq!(at(std::ptr::from_ref(&push.splash_args).cast()), 104, "splashArgs");
        assert_eq!(
            at(std::ptr::from_ref(&push.terrain_heights).cast()),
            112,
            "terrainHeights"
        );
        assert_eq!(size_of::<Push>(), 120, "the whole block");
        assert!(size_of::<Push>() <= 128, "past the guaranteed push-constant size");
    }

    /// **The counts are the shader's**, and nothing but this says so: too few
    /// drops in the buffer and the dispatch reads past the end through a device
    /// address, which is a hang rather than a validation message.
    #[test]
    fn the_buffer_sizes_match_the_shader_s_counts() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/include/rain.slang"
        ))
        .expect("the shared rain header is beside the crate that compiles it");

        let constant = |name: &str| -> u32 {
            let needle = format!("static const uint {name} = ");
            let start = source.find(&needle).unwrap_or_else(|| panic!("no {name}")) + needle.len();
            let rest = &source[start..];
            let end = rest.find(|c: char| !c.is_ascii_digit()).expect("a number");
            rest[..end].parse().expect("a number")
        };

        assert_eq!(constant("RAIN_DROPS"), DROPS);
        assert_eq!(constant("RAIN_SPLASH_RING"), SPLASH_RING);
        assert_eq!(size_of::<Drop>(), 32, "RainDrop is two float4s");
        assert_eq!(size_of::<Splash>(), 32, "RainSplash is two float4s");
    }

    /// **The ring must not lap within one tick**, or which splashes survive
    /// depends on the order the atomics resolved in and the golden gate flakes.
    ///
    /// A drop crosses the 32 m block at about 8 m/s, so roughly one drop in 240
    /// lands per tick.
    #[test]
    fn one_tick_of_landings_cannot_lap_the_splash_ring() {
        let per_tick = DROPS / 240;
        assert!(
            per_tick * SPLASH_STEPS <= SPLASH_RING,
            "{SPLASH_STEPS} ticks of {per_tick} landings overruns a {SPLASH_RING}-entry ring"
        );
        assert!(per_tick < SPLASH_RING, "a single tick laps the ring");
    }
}
