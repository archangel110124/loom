//! The cloud deck, marched once per frame into a direction-indexed map.
//!
//! ADR 0078 Addendum 3 measured the volumetric deck and found the cost was not
//! where the ADR predicted it. At 960x640 on `mood_deep` the forward pass went
//! 0.068 -> 0.368 ms and the **water pass went 0.191 -> 1.66 ms** — 83% of the
//! whole cost — because `waterFragmentMain`'s reflection calls `skyColor`, so
//! the march ran once per water pixel as well as once per background pixel.
//!
//! **That rules out the escalation ADR 0078 §5 named first.** A half-resolution
//! screen-space target optimises the background and can do nothing at all for a
//! reflected ray, which has no screen position. What every consumer *does* have
//! is a direction — so the deck is marched once into an equirectangular map and
//! both the background and every reflected ray read it with one fetch.
//!
//! # Why 1024x512, derived rather than picked
//!
//! `squall` authors the tightest masses in the repository at `cloud_scale =
//! 260`, which at ~3 km subtends about 5 degrees. Texels across one mass:
//!
//! ```text
//!      256x128    1.406 deg/texel     3.5 texels    blobby
//!      512x256    0.703               7.1
//!     1024x512    0.352              14.1          <- this
//!     2048x1024   0.176              28.2
//! ```
//!
//! and the cost, as pixels actually marched:
//!
//! ```text
//!      512x256    0.13 MP        1024x512   0.52 MP        2048x1024  2.10 MP
//!     a 1080p frame of background + water is 2.07 MP
//! ```
//!
//! So 2048 would buy **no saving whatever** against 1080p, and 1024 is a 4x cut
//! with 14 texels across the smallest mass this repository authors. The number
//! that matters is not the ratio though: it is that the deck now costs 0.52 MP
//! of marching **whatever the output resolution and however much water is on
//! screen**, where before it scaled with both.
//!
//! # Upper hemisphere only
//!
//! ADR 0078 §4 records the human's constraint — *"Never — always below the
//! base"* — as the licence for the cheap slab march. The same constraint says
//! the deck is never below the eye, so half a sphere would be half a map spent
//! on ground. `cloudMapUv` maps elevation 0..90 degrees onto the full `v` range,
//! which is a free doubling of vertical resolution.
//!
//! # What it costs in quality, stated
//!
//! 0.352 deg/texel against a 1080p pixel's 0.047 deg — the primary sky is about
//! **7.5x softer** than a per-pixel march. Clouds are low-frequency and it is
//! the trade the human accepted explicitly; it is also measurable, and reversible
//! by marching the background and keeping the map for reflections alone.
//!
//! # Two entry points in `scene.slang`, not a shader of its own
//!
//! `cloudMapVertexMain` and `cloudMapFragmentMain` live in `scene.slang` beside
//! the march they call. `build.rs` passes `-fvk-use-entrypoint-name`, so a module
//! may carry many entry points and a pipeline picks one by name. **That means
//! there is no second implementation of the deck to keep in step** — the map is
//! marched by the same `cloudLookVolume`, reading the same `clouds_at`, through
//! the same push block, as the inline path it replaces.
//!
//! It also reuses the **scene's** pipeline layout rather than declaring one.
//! A superset layout is always valid, the entry point statically uses no
//! descriptor, and borrowing the existing one means there is no second
//! push-constant range that could drift from `Push`.
//!
//! # One type, both renderers
//!
//! `renderer.rs` and `viewer.rs` each own one of these and both call
//! [`CloudMap::record`]. The `render-in-both-paths` skill lists four shipped
//! defects from mirroring draw wiring instead of sharing it — the nappe that
//! existed only offscreen, the cinematic tier the window never drew — and no
//! golden image can see the second path, because the gate only ever renders
//! headless.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::debug_names::DebugNames;
use crate::renderer::{RenderError, create_image, create_shader_module, create_view};

/// Width of the map. See the module header for the derivation.
pub(crate) const CLOUD_MAP_WIDTH: u32 = 1024;

/// Height. Half the width: `u` spans a full turn of azimuth, `v` a quarter turn
/// of elevation, so equal angular resolution on both axes is a 2:1 image.
pub(crate) const CLOUD_MAP_HEIGHT: u32 = 512;

/// **Float, not `R8G8B8A8`.** The map stores linear radiance, and the whole
/// renderer is linear and unbounded above 1.0 since the tonemap landed. An
/// 8-bit target would quantise the deck's small spread of greys — `CLOUD_DARK`
/// to `CLOUD_LIT` is 0.29 to 0.78 — into about 125 levels and band a sky that
/// is nothing but gradient.
pub(crate) const CLOUD_MAP_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

/// The map, its view, and the pipeline that fills it.
///
/// **Field order is not ownership order** — `Drop` below destroys explicitly,
/// which is the lesson of `e40a6b1`, where an `Option<(Instance, Device)>`
/// dropped the instance before the device and segfaulted inside the driver on
/// every window close.
pub(crate) struct CloudMap {
    device: ash::Device,
    image: vk::Image,
    view: vk::ImageView,
    allocation: Option<Allocation>,
    pipeline: vk::Pipeline,
}

impl CloudMap {
    /// Build the target and the pipeline.
    ///
    /// `layout` is the scene's pipeline layout, borrowed rather than owned:
    /// this type never destroys it.
    pub(crate) fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        cache: vk::PipelineCache,
        layout: vk::PipelineLayout,
        names: &DebugNames,
    ) -> Result<Self, RenderError> {
        let (image, allocation) = create_image(
            device,
            allocator,
            CLOUD_MAP_WIDTH,
            CLOUD_MAP_HEIGHT,
            CLOUD_MAP_FORMAT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            1,
            vk::SampleCountFlags::TYPE_1,
            "loom.cloud_map",
        )?;
        let view = create_view(device, image, CLOUD_MAP_FORMAT, vk::ImageAspectFlags::COLOR)?;
        names.set(image, "loom.cloud_map");
        names.set(view, "loom.cloud_map.view");

        let pipeline = match cloud_map_pipeline(device, cache, layout) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                // SAFETY: neither object has been handed to a command buffer.
                unsafe {
                    device.destroy_image_view(view, None);
                    device.destroy_image(image, None);
                }
                let _ = allocator.free(allocation);
                return Err(error);
            }
        };
        names.set(pipeline, "loom.cloud_map.pipeline");

        Ok(Self {
            device: device.clone(),
            image,
            view,
            allocation: Some(allocation),
            pipeline,
        })
    }

    pub(crate) fn image(&self) -> vk::Image {
        self.image
    }

    pub(crate) fn view(&self) -> vk::ImageView {
        self.view
    }

    /// March the deck into the map.
    ///
    /// **Declares `ColorWrite` to the graph and records a colour write**, which
    /// is the rule `573cbca` cost a `SYNC-HAZARD-WRITE-AFTER-WRITE` to learn:
    /// what a pass declares must match what it records.
    ///
    /// **Set 3 is bound even though this entry point samples nothing from it**,
    /// and that was not the first guess. The reasoning was that
    /// `cloudMapFragmentMain` reads only the push block, so no descriptor need
    /// be bound — and the validation layers refused it flatly:
    ///
    /// ```text
    /// VUID-vkCmdDraw-None-08600: The VkPipeline statically uses descriptor
    /// set 3, but because a descriptor was never bound, the VkPipelineLayouts
    /// are not compatible.
    /// ```
    ///
    /// Slang compiles `scene.slang` as one module and the set counts as used.
    /// `renderer.rs` already carries the same note for the water draw — *"Set 3
    /// as well, because the water fragment shader statically samples it"* — so
    /// this is the established shape here, not a workaround.
    ///
    /// Binding it points a sampler at the very image this pass is writing. That
    /// is safe only because nothing in this entry point reads it: the descriptor
    /// is bound, never accessed. `cloudLook` — the function that *would* sample
    /// it — is deliberately not on this path.
    ///
    /// # Safety
    ///
    /// The graph must have put the map in `COLOR_ATTACHMENT_OPTIMAL`, and `cmd`
    /// must be recording outside any rendering block.
    pub(crate) unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        push: &crate::renderer::Push,
        sets: &[(u32, vk::DescriptorSet)],
    ) {
        let extent = vk::Extent2D { width: CLOUD_MAP_WIDTH, height: CLOUD_MAP_HEIGHT };
        let attachment = vk::RenderingAttachmentInfo::default()
            .image_view(self.view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            // CLEAR rather than DONT_CARE: every texel is written by the
            // triangle, but a load op the driver can elide is cheaper than one
            // it must preserve, and an undefined attachment is the kind of
            // thing that reads fine until a tiled GPU sees it.
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] },
            });
        let attachments = [attachment];
        let rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent })
            .layer_count(1)
            .color_attachments(&attachments);

        let viewport = vk::Viewport::default()
            .width(CLOUD_MAP_WIDTH as f32)
            .height(CLOUD_MAP_HEIGHT as f32)
            .max_depth(1.0);
        let scissor = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent };

        // SAFETY: the caller's contract covers the layout and the recording
        // state; every borrowed struct outlives the calls.
        unsafe {
            device.cmd_begin_rendering(cmd, &rendering);
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor(cmd, 0, &[scissor]);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            for (index, set) in sets {
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    layout,
                    *index,
                    &[*set],
                    &[],
                );
            }
            let bytes = std::slice::from_raw_parts(
                std::ptr::from_ref(push).cast::<u8>(),
                size_of::<crate::renderer::Push>(),
            );
            device.cmd_push_constants(
                cmd,
                layout,
                crate::renderer::PUSH_STAGES,
                0,
                bytes,
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_rendering(cmd);
        }
    }

    /// Give the allocation back. Called before the allocator is dropped.
    pub(crate) fn free(&mut self, allocator: &mut Allocator) {
        if let Some(allocation) = self.allocation.take() {
            let _ = allocator.free(allocation);
        }
    }
}

impl Drop for CloudMap {
    fn drop(&mut self) {
        // SAFETY: the caller has already waited for the device to be idle.
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
        }
    }
}

/// The pipeline: a full-screen triangle into the map, single-sampled.
///
/// **`TYPE_1`, and that is a correctness requirement rather than a choice** — a
/// pipeline's rasterisation sample count must equal its attachment's, and the
/// map is a plain single-sample image. Getting it wrong is four validation
/// errors rather than a visual bug.
fn cloud_map_pipeline(
    device: &ash::Device,
    cache: vk::PipelineCache,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, RenderError> {
    let module = create_shader_module(device, crate::SCENE_SPV)?;

    let vertex_entry = c"cloudMapVertexMain";
    let fragment_entry = c"cloudMapFragmentMain";
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
    let viewport_state =
        vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
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

    let color_formats = [CLOUD_MAP_FORMAT];
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
