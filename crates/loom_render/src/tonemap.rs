//! The pass where the frame's range collapses.
//!
//! Everything upstream is linear `R16G16B16A16_SFLOAT` with no ceiling. This
//! reads it once, applies an exposure and a shoulder, and writes the
//! display-referred result into an `_SRGB` attachment so the hardware does the
//! encode exactly as it always has. What moved is WHERE — from the first
//! fragment write to the last.
//!
//! **Shaped after `cmaa2.rs` deliberately**, with the sampler and the owned
//! intermediate removed: one descriptor set layout, one pool sized for one
//! set, one pipeline layout with a four-byte push range, one pipeline. Every
//! fallible path after the first pipeline tears down what came before, because
//! the object-tracking layer otherwise reports the leak instead of the real
//! error.

use ash::vk;

use crate::debug_names::DebugNames;
use crate::renderer::{RenderError, create_shader_module};

/// What a scene that says nothing about exposure gets.
///
/// **Unit, and that is the acceptance test rather than a taste call.** The
/// shoulder is identity below its knee, so at this value every frame with
/// nothing above the knee in it is bit-identical to what the renderer produced
/// before this pass existed.
pub(crate) const DEFAULT_EXPOSURE: f32 = 1.0;

pub(crate) struct Tonemap {
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl Tonemap {
    /// Build the pass for a destination of `color_format`, reading `source`.
    ///
    /// The format is a parameter for the same reason `Cmaa2` takes one: the
    /// offscreen path lands in `R8G8B8A8_SRGB` and the window presents
    /// `B8G8R8A8_SRGB`, and dynamic rendering bakes the attachment format into
    /// the pipeline, so the two cannot share.
    pub(crate) fn new(
        device: &ash::Device,
        names: &DebugNames,
        cache: vk::PipelineCache,
        color_format: vk::Format,
        source: vk::ImageView,
    ) -> Result<Self, RenderError> {
        // **`SAMPLED_IMAGE`, not `COMBINED_IMAGE_SAMPLER`.** This is a
        // one-to-one copy: there is nothing to filter, the shader uses `Load`,
        // and a sampler would be one more object to create, name and destroy
        // for no pixel.
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        // SAFETY: `bindings` outlives the call.
        let layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }?;

        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(1)];
        // SAFETY: `sizes` outlives the call.
        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default().pool_sizes(&sizes).max_sets(1),
                None,
            )
        }?;

        let set_layouts = [layout];
        // SAFETY: `set_layouts` outlives the call and the pool holds one set.
        let set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&set_layouts),
            )
        }?[0];

        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(4)];
        // SAFETY: both slices outlive the call.
        let pipeline_layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&ranges),
                None,
            )
        }?;

        let mut this = Self { layout, pool, set, pipeline_layout, pipeline: vk::Pipeline::null() };

        match create_pipeline(device, pipeline_layout, cache, color_format) {
            Ok(pipeline) => this.pipeline = pipeline,
            Err(e) => {
                // SAFETY: nothing has been recorded against these yet.
                unsafe { this.destroy(device) };
                return Err(e);
            }
        }
        names.set(this.pipeline, "loom.tonemap_pipeline");

        this.rebind(device, source);
        Ok(this)
    }

    /// Point the set at the scene image. Called at construction and again on
    /// every resize, because the image is recreated then and the old view is
    /// destroyed under the descriptor.
    ///
    /// **Never per frame.** A descriptor write per frame is a write the
    /// validation layers cannot tell apart from a mistake.
    pub(crate) fn rebind(&self, device: &ash::Device, source: vk::ImageView) {
        let info = [vk::DescriptorImageInfo::default()
            .image_view(source)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&info)];
        // SAFETY: `source` is live and `info` outlives the call.
        unsafe { device.update_descriptor_sets(&writes, &[]) };
    }

    /// Record the collapse into `cmd`.
    ///
    /// # Safety
    /// `cmd` must be recording outside any rendering block, the source must be
    /// in `SHADER_READ_ONLY_OPTIMAL` and the destination in
    /// `COLOR_ATTACHMENT_OPTIMAL`.
    pub(crate) unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        destination: vk::ImageView,
        exposure: f32,
        width: u32,
        height: u32,
    ) {
        // No depth attachment: there is no geometry to test. The pipeline
        // declares `UNDEFINED` for depth to match, because a pipeline that
        // disagrees with its rendering info is a validation error on every
        // draw.
        let attachments = [vk::RenderingAttachmentInfo::default()
            .image_view(destination)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            // Every pixel is written, so loading the previous contents would
            // be bandwidth spent on values about to be overwritten.
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)];
        let rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D { width, height },
            })
            .layer_count(1)
            .color_attachments(&attachments);

        #[allow(clippy::cast_precision_loss)]
        let (fw, fh) = (width as f32, height as f32);

        // SAFETY: the caller guarantees the layouts and that `cmd` is
        // recording; every slice outlives its call.
        unsafe {
            device.cmd_begin_rendering(cmd, &rendering);
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: fw,
                    height: fh,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D { width, height },
                }],
            );
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                &exposure.to_ne_bytes(),
            );
            // One oversized triangle rather than a quad: a quad is two
            // triangles with a seam down the diagonal where the rasteriser
            // evaluates the same pixels twice.
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_rendering(cmd);
        }
    }

    /// # Safety
    /// The device must be idle.
    pub(crate) unsafe fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: the caller has idled the device, so nothing references these.
        unsafe {
            if self.pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.pipeline, None);
            }
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}

fn create_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    color_format: vk::Format,
) -> Result<vk::Pipeline, RenderError> {
    let module = create_shader_module(device, crate::TONEMAP_SPV)?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(c"tonemapVertexMain"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(c"tonemapFragmentMain"),
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(false)
        .color_write_mask(vk::ColorComponentFlags::RGBA)];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [color_format];
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(vk::Format::UNDEFINED);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport)
        .rasterization_state(&raster)
        .multisample_state(&multisample)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering);

    // SAFETY: every borrowed slice outlives the call.
    let result = unsafe { device.create_graphics_pipelines(cache, &[info], None) };
    // SAFETY: the module is consumed by pipeline creation whether or not it
    // succeeded.
    unsafe { device.destroy_shader_module(module, None) };

    match result {
        Ok(pipelines) => Ok(pipelines[0]),
        Err((_, e)) => Err(RenderError::from(e)),
    }
}
