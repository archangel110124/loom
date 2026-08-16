//! The CMAA2-class full-screen anti-aliasing pass authorised by ADR 0010.
//!
//! `assets/shaders/cmaa2.slang` and `assets/shaders/cmaa2_edges.slang` document
//! exactly which parts of Intel's algorithm are implemented and which are not;
//! read those before trusting the name. This file is only the plumbing.
//!
//! # Two passes, because the edge mask has to be stored
//!
//! It began as one fragment pass that recomputed a pixel's edges — and its four
//! neighbours' — from the colour image, five overlapping evaluations per pixel.
//! That is workable for the simple-shape blend, which never looks further than
//! one pixel away, and impossible for the Z-shape path, which walks along an
//! edge for dozens of pixels. Sixteen texture fetches per step of that walk is
//! not a cost worth paying when one will do.
//!
//! So pass 1 writes the four edge bits per pixel into an `R8_UINT` image, and
//! pass 2 reads it. The split also made the *first* pass cheaper: a 4x4 luma
//! window instead of 6x6, and each edge computed once.
//!
//! # Why fragment passes and not compute
//!
//! The obvious shape for a post-process is a compute dispatch writing a storage
//! image. It is the wrong shape *here*, for a reason that is a property of this
//! renderer rather than a preference:
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
//! The edge image *could* be a storage image — `R8_UINT` supports it — but
//! there is nothing to gain: it is written once per pixel from a full-screen
//! triangle either way, and a second dispatch shape would mean a second set of
//! barriers to reason about for no measured difference.
//!
//! It also needs no new [`Access`](loom_render_graph::Access) variant: the
//! graph already models `ShaderRead` and `ColorWrite`, which is exactly what
//! these passes do to three images.
//!
//! The cost of the choice is that **Intel's Z blend scatters and this one
//! gathers** — see the shader header. That is the one algorithmic difference,
//! and it is forced by exactly the constraint above.
//!
//! # Three images, and never fewer
//!
//! The blend pass reads a neighbourhood and writes the centre, so reading and
//! writing the same image would have every pixel racing its neighbours — the
//! output of one texel depending on whether another had been rewritten yet.
//! The destination is always a different image, and all three go through the
//! render graph (never-do #4).

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::debug_names::DebugNames;
use crate::renderer::{RenderError, create_image, create_shader_module, create_view};

/// The packed edge mask. Four bits used of eight; there is no smaller colour
/// format, and packing two pixels per byte the way Intel's
/// `CMAA_PACK_SINGLE_SAMPLE_EDGE_TO_HALF_WIDTH` does would buy bandwidth this
/// pass is nowhere near limited by.
const EDGE_FORMAT: vk::Format = vk::Format::R8_UINT;

/// Whether the pass runs. **On by default**; `LOOM_CMAA2=0` turns it off.
///
/// It earned that by measurement rather than by having been authorised. ADR
/// 0010 accepted it on the inference that smoothing a sub-pixel edge would
/// steady it frame to frame, and `cargo xtask shimmer` says the inference holds
/// — modestly, and with a shape worth understanding:
///
/// ```text
///                          meadow   grass_slope
///     4x MSAA, no pass      3.059      1.755
///     4x + simple shapes    2.791      1.605
///     4x + Z-shapes too     2.788      1.605
///     8x MSAA, no pass      2.824      1.310
///     8x + simple shapes    2.601      1.244
///     8x + Z-shapes too     2.598      1.244
/// ```
///
/// **The Z-shape path is worth nothing on grass, and that is the measurement,
/// not a shrug.** 0.003 on `meadow` and exactly zero on `grass_slope` — both
/// inside the metric's noise. It was predicted: Z-shape handling targets long,
/// shallow-angled edges, and a field of thin near-vertical blades has none.
/// What that field has is sub-pixel *coverage* flicker, where the information
/// is missing from the frame rather than merely mis-shaped, and no spatial
/// filter can invent it. The path is not inert — it changes 0.037% of
/// `primitives` and 0.044% of `materials` at 1080p, and on a long shallow
/// silhouette it visibly replaces a staircase with a ramp — it simply has
/// nothing to work on in grass.
///
/// The reason the pass is on at all: **4x + this (2.788) beats 8x MSAA alone
/// (2.824)** for a fraction of a millisecond rather than for double the MSAA
/// bandwidth. It is the better buy at the margin, which is a different and much
/// smaller claim than "it fixes the aliasing" — it does not. `meadow` sits at
/// 2.788 against a 0.000 control and Phase 2's exit criterion 2 remains **not
/// met**.
///
/// An environment variable rather than a `const bool` for the same reason
/// `LOOM_GPU_TIMING` is one: it makes "the same scene, with and without" a pair
/// of commands rather than a pair of builds, which is what the measurement
/// needs. `LOOM_CMAA2=0` is how you take the reference numbers.
#[must_use]
pub(crate) fn requested() -> bool {
    std::env::var_os("LOOM_CMAA2").is_none_or(|v| v != "0")
}

/// Both passes: two pipelines, two descriptor sets, and the edge image between
/// them.
pub(crate) struct Cmaa2 {
    edge_layout: vk::DescriptorSetLayout,
    blend_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    edge_set: vk::DescriptorSet,
    blend_set: vk::DescriptorSet,
    sampler: vk::Sampler,
    edge_pipeline_layout: vk::PipelineLayout,
    blend_pipeline_layout: vk::PipelineLayout,
    edge_pipeline: vk::Pipeline,
    blend_pipeline: vk::Pipeline,
    edges: vk::Image,
    edges_view: vk::ImageView,
    edges_alloc: Option<Allocation>,
}

impl Cmaa2 {
    /// Build both passes for a destination of `color_format`, reading `source`.
    ///
    /// The format is a parameter because the offscreen path resolves into
    /// `R8G8B8A8_SRGB` and the window presents `B8G8R8A8_SRGB`; dynamic
    /// rendering bakes the attachment format into the pipeline, so the two
    /// cannot share one. The edge pass's format never varies, so it does not
    /// take one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        names: &DebugNames,
        cache: vk::PipelineCache,
        color_format: vk::Format,
        source: vk::ImageView,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        // **Two layouts, not one with an unused binding.** The edge pass has no
        // edge image to read, and giving it a descriptor pointing at one would
        // mean a set claiming `SHADER_READ_ONLY_OPTIMAL` for an image that is a
        // colour attachment at that moment. Whether validation objects depends
        // on whether it counts the binding as statically used, which is not a
        // thing to be clever about.
        let sampled = |ty, stage| {
            vk::DescriptorSetLayoutBinding::default()
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(stage)
        };
        let edge_bindings =
            [sampled(vk::DescriptorType::COMBINED_IMAGE_SAMPLER, vk::ShaderStageFlags::FRAGMENT)
                .binding(0)];
        let blend_bindings = [
            sampled(vk::DescriptorType::COMBINED_IMAGE_SAMPLER, vk::ShaderStageFlags::FRAGMENT)
                .binding(0),
            sampled(vk::DescriptorType::SAMPLED_IMAGE, vk::ShaderStageFlags::FRAGMENT).binding(1),
        ];
        // SAFETY: the binding slices outlive the calls.
        let (edge_layout, blend_layout) = unsafe {
            (
                device.create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&edge_bindings),
                    None,
                )?,
                device.create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&blend_bindings),
                    None,
                )?,
            )
        };

        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(2),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(&sizes)
            .max_sets(2);
        // SAFETY: `sizes` outlives the call.
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }?;

        let layouts = [edge_layout, blend_layout];
        let allocate = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: `layouts` outlives the call and the pool has room for both.
        let sets = unsafe { device.allocate_descriptor_sets(&allocate) }?;
        let (edge_set, blend_set) = (sets[0], sets[1]);

        // **NEAREST and CLAMP_TO_EDGE, both load-bearing.** The shaders read
        // texels, not a filtered image: a linear filter would silently average
        // the neighbours the edge detection is trying to compare, and it would
        // find a softened version of its own input. Clamping makes a read past
        // the border return the border pixel, so the luma window needs no
        // bounds test in the inner loop.
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: no borrowed data in the info.
        let sampler = unsafe { device.create_sampler(&sampler_info, None) }?;

        let edge_set_layouts = [edge_layout];
        let blend_set_layouts = [blend_layout];
        // SAFETY: both slices outlive their calls.
        let (edge_pipeline_layout, blend_pipeline_layout) = unsafe {
            (
                device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&edge_set_layouts),
                    None,
                )?,
                device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&blend_set_layouts),
                    None,
                )?,
            )
        };

        let mut this = Self {
            edge_layout,
            blend_layout,
            pool,
            edge_set,
            blend_set,
            sampler,
            edge_pipeline_layout,
            blend_pipeline_layout,
            edge_pipeline: vk::Pipeline::null(),
            blend_pipeline: vk::Pipeline::null(),
            edges: vk::Image::null(),
            edges_view: vk::ImageView::null(),
            edges_alloc: None,
        };

        // From here on, every failure path has to tear down what came before —
        // `field_agree.rs` documents what happens when one does not: the
        // object-tracking check at teardown reports the leak instead of the
        // real error.
        let pipelines = create_pipeline(
            device,
            edge_pipeline_layout,
            cache,
            EDGE_FORMAT,
            crate::CMAA2_EDGES_SPV,
            c"edgeVertexMain",
            c"edgeFragmentMain",
        )
        .and_then(|edge| {
            create_pipeline(
                device,
                blend_pipeline_layout,
                cache,
                color_format,
                crate::CMAA2_SPV,
                c"aaVertexMain",
                c"aaFragmentMain",
            )
            .map(|blend| (edge, blend))
        });
        match pipelines {
            Ok((edge, blend)) => {
                this.edge_pipeline = edge;
                this.blend_pipeline = blend;
            }
            Err(e) => {
                // SAFETY: nothing recorded against any of this yet.
                unsafe { this.destroy(device, allocator) };
                return Err(e);
            }
        }

        // SAFETY: the device is idle — nothing has been submitted against a set
        // that does not exist yet.
        if let Err(e) = unsafe { this.rebind(device, allocator, names, source, width, height) } {
            // SAFETY: as above.
            unsafe { this.destroy(device, allocator) };
            return Err(e);
        }

        names.set(this.edge_pipeline, "loom.cmaa2_edge_pipeline");
        names.set(this.blend_pipeline, "loom.cmaa2_pipeline");
        names.set(sampler, "loom.cmaa2_sampler");
        Ok(this)
    }

    /// The edge mask image, for the caller to import into the render graph.
    pub(crate) fn edges_image(&self) -> vk::Image {
        self.edges
    }

    /// Point the passes at the image they should anti-alias, sizing the edge
    /// mask to match.
    ///
    /// Called once at creation and again after a resize. **Never per frame**:
    /// the sets are the same sets every frame and rewriting them would be the
    /// per-draw descriptor allocation never-do #2 forbids, one step removed.
    ///
    /// # Safety
    /// No command buffer referencing these sets or the edge image may be in
    /// flight.
    pub(crate) unsafe fn rebind(
        &mut self,
        device: &ash::Device,
        allocator: &mut Allocator,
        names: &DebugNames,
        source: vk::ImageView,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        // SAFETY: the caller guarantees nothing is in flight.
        unsafe { self.destroy_edges(device, allocator) };

        let (edges, allocation) = create_image(
            device,
            allocator,
            width,
            height,
            EDGE_FORMAT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            1,
            vk::SampleCountFlags::TYPE_1,
            "loom.aa_edges",
        )?;
        let edges_view = create_view(device, edges, EDGE_FORMAT, vk::ImageAspectFlags::COLOR)?;
        names.set(edges, "loom.aa_edges");
        names.set(edges_view, "loom.aa_edges.view");
        self.edges = edges;
        self.edges_view = edges_view;
        self.edges_alloc = Some(allocation);

        let source_info = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler)
            .image_view(source)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let edge_info = [vk::DescriptorImageInfo::default()
            .image_view(edges_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.edge_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&source_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.blend_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&source_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.blend_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&edge_info),
        ];
        // SAFETY: the infos outlive the call and the caller guarantees the sets
        // are not in use.
        unsafe { device.update_descriptor_sets(&writes, &[]) };
        Ok(())
    }

    /// Record pass 1 — edge detection into the mask image — into `cmd`.
    ///
    /// # Safety
    /// `cmd` must be recording outside any rendering block, the source image
    /// must already be in `SHADER_READ_ONLY_OPTIMAL` and the edge image in
    /// `COLOR_ATTACHMENT_OPTIMAL` — which is the graph's job, not this one's.
    pub(crate) unsafe fn record_edges(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        width: u32,
        height: u32,
    ) {
        // SAFETY: the caller guarantees the layouts and that `cmd` is recording.
        unsafe {
            self.draw(
                device,
                cmd,
                self.edges_view,
                self.edge_pipeline,
                self.edge_pipeline_layout,
                self.edge_set,
                width,
                height,
            );
        }
    }

    /// Record pass 2 — the shape filter — into `cmd`.
    ///
    /// # Safety
    /// `cmd` must be recording outside any rendering block, the source and edge
    /// images must already be in `SHADER_READ_ONLY_OPTIMAL` and the destination
    /// in `COLOR_ATTACHMENT_OPTIMAL`.
    pub(crate) unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        destination: vk::ImageView,
        width: u32,
        height: u32,
    ) {
        // SAFETY: the caller guarantees the layouts and that `cmd` is recording.
        unsafe {
            self.draw(
                device,
                cmd,
                destination,
                self.blend_pipeline,
                self.blend_pipeline_layout,
                self.blend_set,
                width,
                height,
            );
        }
    }

    /// One full-screen triangle into one colour attachment. Both passes are
    /// this, and writing it twice is how the two drift apart.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        destination: vk::ImageView,
        pipeline: vk::Pipeline,
        layout: vk::PipelineLayout,
        set: vk::DescriptorSet,
        width: u32,
        height: u32,
    ) {
        // No depth attachment: these passes have no geometry to test. The
        // pipeline declares `UNDEFINED` for depth to match, because a pipeline
        // that disagrees with its rendering info is a validation error on every
        // draw.
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
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                layout,
                0,
                &[set],
                &[],
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_rendering(cmd);
        }
    }

    /// # Safety
    /// No command buffer referencing the edge image may be in flight.
    unsafe fn destroy_edges(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        // SAFETY: the caller guarantees nothing is in flight.
        unsafe {
            if self.edges_view != vk::ImageView::null() {
                device.destroy_image_view(self.edges_view, None);
            }
            if self.edges != vk::Image::null() {
                device.destroy_image(self.edges, None);
            }
        }
        self.edges_view = vk::ImageView::null();
        self.edges = vk::Image::null();
        if let Some(allocation) = self.edges_alloc.take() {
            let _ = allocator.free(allocation);
        }
    }

    /// # Safety
    /// The device must be idle.
    pub(crate) unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        // SAFETY: the caller has idled the device and these handles are ours.
        unsafe {
            self.destroy_edges(device, allocator);
            if self.blend_pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.blend_pipeline, None);
            }
            if self.edge_pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.edge_pipeline, None);
            }
            device.destroy_pipeline_layout(self.blend_pipeline_layout, None);
            device.destroy_pipeline_layout(self.edge_pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.blend_layout, None);
            device.destroy_descriptor_set_layout(self.edge_layout, None);
        }
        self.blend_pipeline = vk::Pipeline::null();
        self.edge_pipeline = vk::Pipeline::null();
    }
}

/// A full-screen triangle pipeline: no vertex input, no depth, one sample.
///
/// One sample even when the scene rasterised at four — these run on the
/// *resolved* image, which is single-sampled by definition.
#[allow(clippy::too_many_arguments)]
fn create_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    color_format: vk::Format,
    spv: &[u8],
    vertex_entry: &std::ffi::CStr,
    fragment_entry: &std::ffi::CStr,
) -> Result<vk::Pipeline, RenderError> {
    let module = create_shader_module(device, spv)?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(vertex_entry),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(fragment_entry),
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
