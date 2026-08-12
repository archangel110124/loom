//! The CMAA2-class full-screen anti-aliasing pass authorised by ADR 0010.
//!
//! `assets/shaders/cmaa2.slang` documents exactly which half of Intel's
//! algorithm is implemented and which half is not; read that before trusting
//! the name. This file is only the plumbing: one descriptor holding the frame
//! that was just rendered, one full-screen triangle, one pipeline.
//!
//! # Why a fragment pass and not compute
//!
//! The obvious shape for a post-process is a compute dispatch writing a
//! storage image. It is the wrong shape *here*, for a reason that is a
//! property of this renderer rather than a preference:
//!
//! `COLOR_FORMAT` is `R8G8B8A8_SRGB` and the swapchain is `B8G8R8A8_SRGB`.
//! **No sRGB format supports `STORAGE`**, so a compute pass could not write
//! either of them. It would need a mutable-format image with a second UNORM
//! view, plus — in the viewer — an extra copy into the swapchain image, which
//! cannot carry `STORAGE` usage at all. A fragment pass writes an sRGB
//! attachment directly, so the hardware does the encode it already does for
//! every other pass, and the viewer's swapchain image is written in place
//! rather than copied into.
//!
//! It also needs no new [`Access`](loom_render_graph::Access) variant: the
//! graph already models `ShaderRead` and `ColorWrite`, which is exactly what
//! this pass does to two images.
//!
//! The cost of the choice is that the pass has no cheap way to keep an edge
//! bitmask between two halves, which is what CMAA2's Z-shape tracing needs.
//! That is the omission the shader documents.
//!
//! # Two images, never one
//!
//! The pass reads a neighbourhood and writes the centre, so reading and
//! writing the same image would have every pixel racing its neighbours — the
//! output of one texel depending on whether another had been rewritten yet.
//! The destination is always a different image, and both go through the render
//! graph (never-do #4).

use ash::vk;

use crate::debug_names::DebugNames;
use crate::renderer::{RenderError, create_shader_module};

/// Whether the pass runs. **On by default**; `LOOM_CMAA2=0` turns it off.
///
/// It earned that by measurement rather than by having been authorised. ADR
/// 0010 accepted it on the inference that smoothing a sub-pixel edge would
/// steady it frame to frame, and `cargo xtask shimmer` says the inference holds
/// — modestly, and with a shape worth understanding:
///
/// ```text
///                meadow    +CMAA2
///     1x MSAA     4.375  ->  3.652   (-16.5%)
///     4x MSAA     3.059  ->  2.791   ( -8.8%)
///     8x MSAA     2.824  ->  2.601   ( -7.9%)
/// ```
///
/// **The filter's share shrinks the more MSAA has already done**, which is what
/// you would expect from what the two things actually are: MSAA recovers
/// information by taking more samples, and this only reshapes what is already
/// in the frame. It is not a substitute for samples and the numbers say so.
///
/// The reason it is on: **4x + this (2.791) beats 8x MSAA alone (2.824)** for
/// 0.042 ms rather than for double the MSAA bandwidth. It is the better buy at
/// the margin, which is a different and much smaller claim than "it fixes the
/// aliasing" — it does not. `meadow` sits at 2.791 against a 0.000 control and
/// Phase 2's exit criterion 2 remains **not met**.
///
/// An environment variable rather than a `const bool` for the same reason
/// `LOOM_GPU_TIMING` is one: it makes "the same scene, with and without" a pair
/// of commands rather than a pair of builds, which is what the measurement
/// needs. `LOOM_CMAA2=0` is how you take the reference numbers.
#[must_use]
pub(crate) fn requested() -> bool {
    std::env::var_os("LOOM_CMAA2").is_none_or(|v| v != "0")
}

/// The pass: a sampler, a set holding the source image, and a pipeline.
pub(crate) struct Cmaa2 {
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl Cmaa2 {
    /// Build the pass for a destination of `color_format`.
    ///
    /// The format is a parameter because the offscreen path resolves into
    /// `R8G8B8A8_SRGB` and the window presents `B8G8R8A8_SRGB`; dynamic
    /// rendering bakes the attachment format into the pipeline, so the two
    /// cannot share one.
    pub(crate) fn new(
        device: &ash::Device,
        names: &DebugNames,
        cache: vk::PipelineCache,
        color_format: vk::Format,
    ) -> Result<Self, RenderError> {
        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let bindings = [binding];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: `bindings` outlives the call.
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }?;

        let size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1);
        let sizes = [size];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(&sizes)
            .max_sets(1);
        // SAFETY: `sizes` outlives the call.
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }?;

        let layouts = [layout];
        let allocate = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: `layouts` outlives the call and the pool has room for one set.
        let set = unsafe { device.allocate_descriptor_sets(&allocate) }?[0];

        // **NEAREST and CLAMP_TO_EDGE, both load-bearing.** The shader reads
        // texels, not a filtered image: a linear filter would silently average
        // the neighbours it is trying to compare, and the edge detection would
        // find a softened version of its own input. Clamping makes a read past
        // the border return the border pixel, so the 6x6 window needs no bounds
        // test in the inner loop.
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: no borrowed data in the info.
        let sampler = unsafe { device.create_sampler(&sampler_info, None) }?;

        let set_layouts = [layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
        // SAFETY: `set_layouts` outlives the call.
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }?;

        let pipeline = match create_aa_pipeline(device, pipeline_layout, cache, color_format) {
            Ok(pipeline) => pipeline,
            Err(e) => {
                // The failure path cleans up too — `field_agree.rs` documents
                // what happens when it does not: the object-tracking check at
                // teardown reports the leak instead of the real error.
                // SAFETY: everything below was created here.
                unsafe {
                    device.destroy_pipeline_layout(pipeline_layout, None);
                    device.destroy_sampler(sampler, None);
                    device.destroy_descriptor_pool(pool, None);
                    device.destroy_descriptor_set_layout(layout, None);
                }
                return Err(e);
            }
        };

        names.set(pipeline, "loom.cmaa2_pipeline");
        names.set(sampler, "loom.cmaa2_sampler");

        Ok(Self {
            layout,
            pool,
            set,
            sampler,
            pipeline_layout,
            pipeline,
        })
    }

    /// Point the pass at the image it should anti-alias.
    ///
    /// Called once at creation and again after a resize. **Never per frame**:
    /// the set is the same set every frame and rewriting it would be the
    /// per-draw descriptor allocation never-do #2 forbids, one step removed.
    ///
    /// # Safety
    /// No command buffer referencing this set may be in flight.
    pub(crate) unsafe fn set_source(&self, device: &ash::Device, view: vk::ImageView) {
        let info = vk::DescriptorImageInfo::default()
            .sampler(self.sampler)
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let infos = [info];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .dst_array_element(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&infos);
        // SAFETY: `infos` outlives the call and the caller guarantees the set
        // is not in use.
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Record the pass into `cmd`.
    ///
    /// # Safety
    /// `cmd` must be recording outside any rendering block, the source image
    /// must already be in `SHADER_READ_ONLY_OPTIMAL` and the destination in
    /// `COLOR_ATTACHMENT_OPTIMAL` — which is the graph's job, not this one's.
    pub(crate) unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        destination: vk::ImageView,
        width: u32,
        height: u32,
    ) {
        // No depth attachment: this pass has no geometry to test. The pipeline
        // declares `UNDEFINED` for depth to match, because a pipeline that
        // disagrees with its rendering info is a validation error on every draw.
        let attachment = vk::RenderingAttachmentInfo::default()
            .image_view(destination)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            // Every pixel is written, so loading the previous contents would be
            // bandwidth spent on values that are about to be overwritten.
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE);
        let attachments = [attachment];
        let rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D { width, height },
            })
            .layer_count(1)
            .color_attachments(&attachments);

        // SAFETY: the caller guarantees the layouts and that `cmd` is recording.
        unsafe {
            device.cmd_begin_rendering(cmd, &rendering);
            crate::renderer::set_viewport(device, cmd, width, height);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_rendering(cmd);
        }
    }

    /// # Safety
    /// The device must be idle.
    pub(crate) unsafe fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: the caller has idled the device and these handles are ours.
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}

/// The pipeline: a full-screen triangle, no vertex input, no depth, one sample.
///
/// One sample even when the scene rasterised at four — this runs on the
/// *resolved* image, which is single-sampled by definition.
fn create_aa_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    color_format: vk::Format,
) -> Result<vk::Pipeline, RenderError> {
    let module = create_shader_module(device, crate::CMAA2_SPV)?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(c"aaVertexMain"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(c"aaFragmentMain"),
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false);
    let attachments = [blend_attachment];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [color_format];
    let mut rendering_info =
        vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering_info);

    // SAFETY: every borrowed struct above outlives this call.
    let created = unsafe { device.create_graphics_pipelines(cache, &[info], None) };
    // SAFETY: baked into the pipeline on success, and useless on failure.
    unsafe { device.destroy_shader_module(module, None) };
    Ok(created.map_err(|(_, r)| RenderError::Vulkan(r))?[0])
}
