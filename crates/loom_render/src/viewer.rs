//! Windowed presentation: swapchain, per-frame sync, resize.
//!
//! This is M5 work pulled forward. The ordering brief §7.1 insists on — headless
//! PNG **before** the swapchain — has already been satisfied, and that ordering
//! was about which path is primary, not about forbidding a window. The offscreen
//! path stays the one `render_preview` uses, so the two cannot diverge.
//!
//! Everything here reuses the offscreen renderer's pipeline, buffers, and
//! shaders; only the target and the synchronisation differ.

use ash::vk;

use crate::debug_names::DebugNames;
use crate::renderer::{
    Camera, DEPTH_FORMAT, Object, ObjectData, RenderError, begin_rendering, create_address_buffer,
    create_image, create_pipeline, create_view, cube_vertices, pack_objects, pipeline_cache_path,
    set_viewport, view_projection, write_slice,
};
use loom_render_graph::{Access, RenderGraph};
use crate::{Device, Instance};
use gpu_allocator::vulkan::{Allocation, Allocator, AllocatorCreateDesc};

const MAX_OBJECTS: usize = 4096;

/// A window's swapchain and everything needed to draw into it.
pub struct Viewer {
    device: ash::Device,
    queue: vk::Queue,
    allocator: Option<Allocator>,

    surface_loader: ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
    swapchain_loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    format: vk::Format,
    extent: vk::Extent2D,
    views: Vec<vk::ImageView>,
    images: Vec<vk::Image>,

    depth: vk::Image,
    depth_view: vk::ImageView,
    depth_alloc: Option<Allocation>,

    vertices: vk::Buffer,
    vertices_alloc: Option<Allocation>,
    vertex_address: vk::DeviceAddress,
    objects: vk::Buffer,
    objects_alloc: Option<Allocation>,
    object_address: vk::DeviceAddress,

    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    pipeline_cache: vk::PipelineCache,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    /// Signalled when the presentation engine hands us an image.
    acquired: vk::Semaphore,
    /// Signalled when rendering finishes, waited on by present.
    rendered: vk::Semaphore,
    in_flight: vk::Fence,
    physical: vk::PhysicalDevice,
}

impl Viewer {
    /// Build a viewer for an existing surface.
    ///
    /// # Errors
    /// [`RenderError`] if any Vulkan object or allocation fails.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        instance: &Instance,
        device: &Device,
        surface: vk::SurfaceKHR,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let raw = device.handle().clone();
        let names = DebugNames::new(instance.handle(), &raw, cfg!(debug_assertions));

        let surface_loader = ash::khr::surface::Instance::new(instance.entry(), instance.handle());
        let swapchain_loader = ash::khr::swapchain::Device::new(instance.handle(), &raw);

        let mut allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.handle().clone(),
            device: raw.clone(),
            physical_device: device.physical(),
            debug_settings: gpu_allocator::AllocatorDebugSettings::default(),
            buffer_device_address: true,
            allocation_sizes: gpu_allocator::AllocationSizes::default(),
        })
        .map_err(|e| RenderError::Allocator(e.to_string()))?;

        let cube = cube_vertices();
        let (vertices, vertices_alloc, vertex_address) = create_address_buffer(
            &raw,
            &mut allocator,
            std::mem::size_of_val(cube.as_slice()) as u64,
            "loom.cube_vertices",
        )?;
        write_slice(&vertices_alloc, &cube)?;

        let (objects, objects_alloc, object_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (MAX_OBJECTS * size_of::<ObjectData>()) as u64,
            "loom.object_data",
        )?;

        let (format, extent, swapchain, images, views) = create_swapchain(
            &raw,
            &surface_loader,
            &swapchain_loader,
            device.physical(),
            surface,
            width,
            height,
            vk::SwapchainKHR::null(),
        )?;

        let (depth, depth_alloc, depth_view) =
            create_depth(&raw, &mut allocator, extent.width, extent.height)?;

        let cache_path = pipeline_cache_path(instance, device);
        // The pipeline is built for the *swapchain's* format, which is usually
        // BGRA rather than the offscreen path's RGBA. Dynamic rendering bakes
        // the attachment format into the pipeline, so this cannot be shared.
        let (pipeline_layout, pipeline, pipeline_cache) =
            create_pipeline(&raw, cache_path.as_deref(), format)?;

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(device.queue_family())
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the family index came from this device.
        let command_pool = unsafe { raw.create_command_pool(&pool_info, None) }?;
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool is live.
        let command_buffer = unsafe { raw.allocate_command_buffers(&alloc_info) }?[0];

        let semaphore_info = vk::SemaphoreCreateInfo::default();
        // Start signalled, so the first frame does not wait on a fence that
        // will never be submitted.
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        // SAFETY: default create-infos are valid.
        let (acquired, rendered, in_flight) = unsafe {
            (
                raw.create_semaphore(&semaphore_info, None)?,
                raw.create_semaphore(&semaphore_info, None)?,
                raw.create_fence(&fence_info, None)?,
            )
        };

        names.set(pipeline, "loom.viewer_pipeline");
        names.set(depth, "loom.viewer_depth");
        names.set(acquired, "loom.sem_image_acquired");
        names.set(rendered, "loom.sem_render_finished");

        Ok(Self {
            device: raw,
            queue: device.queue(),
            allocator: Some(allocator),
            surface_loader,
            surface,
            swapchain_loader,
            swapchain,
            format,
            extent,
            views,
            images,
            depth,
            depth_view,
            depth_alloc: Some(depth_alloc),
            vertices,
            vertices_alloc: Some(vertices_alloc),
            vertex_address,
            objects,
            objects_alloc: Some(objects_alloc),
            object_address,
            pipeline_layout,
            pipeline,
            pipeline_cache,
            command_pool,
            command_buffer,
            acquired,
            rendered,
            in_flight,
            physical: device.physical(),
        })
    }

    /// Draw one frame and present it.
    ///
    /// # Errors
    /// [`RenderError`] on Vulkan failure. A resize is handled internally and
    /// is not an error.
    #[allow(clippy::too_many_lines)]
    pub fn draw(&mut self, objects: &[Object], camera: &Camera) -> Result<(), RenderError> {
        let d = self.device.clone();

        // SAFETY: waiting on the previous frame's fence before touching any
        // per-frame resource is what makes reuse of the command buffer legal.
        unsafe {
            d.wait_for_fences(&[self.in_flight], true, u64::MAX)?;
        }

        // SAFETY: the swapchain is live.
        let acquired = unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                self.acquired,
                vk::Fence::null(),
            )
        };
        let index = match acquired {
            Ok((index, _suboptimal)) => index,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate()?;
                return Ok(());
            }
            Err(e) => return Err(RenderError::Vulkan(e)),
        };

        #[allow(clippy::cast_precision_loss)]
        let aspect = self.extent.width as f32 / self.extent.height as f32;
        let object_data = pack_objects(objects, view_projection(camera, aspect));
        write_slice(
            self.objects_alloc
                .as_ref()
                .ok_or_else(|| RenderError::Allocator("object buffer missing".into()))?,
            &object_data,
        )?;

        let image = self.images[index as usize];
        let view = self.views[index as usize];
        let cmd = self.command_buffer;

        // Barriers belong to the graph here as well (never-do #4). The
        // swapchain image starts UNDEFINED every frame — its contents are not
        // preserved between presents, and pretending otherwise would make the
        // driver keep them for nothing.
        let mut graph = RenderGraph::new();
        let target = graph.import("loom.swapchain_image", image);

        let extent = self.extent;
        let depth_view = self.depth_view;
        let (pipeline, layout) = (self.pipeline, self.pipeline_layout);
        let push = crate::renderer::Push {
            vertices: self.vertex_address,
            objects: self.object_address,
        };
        let instances = u32::try_from(objects.len()).unwrap_or(0);
        let depth_image = self.depth;
        let depth_id = graph.import("loom.viewer_depth", depth_image);

        graph.pass(
            "forward",
            &[(target, Access::ColorWrite), (depth_id, Access::DepthWrite)],
            move |d, cmd| {
                // SAFETY: the graph transitioned both attachments already.
                unsafe {
                    begin_rendering(d, cmd, view, depth_view, extent.width, extent.height);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    set_viewport(d, cmd, extent.width, extent.height);
                    let bytes = std::slice::from_raw_parts(
                        std::ptr::from_ref(&push).cast::<u8>(),
                        size_of::<crate::renderer::Push>(),
                    );
                    d.cmd_push_constants(
                        cmd,
                        layout,
                        vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                        0,
                        bytes,
                    );
                    d.cmd_draw(cmd, 36, instances, 0, 0);
                    d.cmd_end_rendering(cmd);
                }
            },
        );

        // Presentable layout. Declared as a pass with no work, because the
        // transition IS the work — and forgetting it is the classic first
        // swapchain bug that validation catches immediately.
        graph.pass("present", &[(target, Access::Present)], |_, _| {});

        // SAFETY: the fence above guarantees this buffer is no longer in flight.
        unsafe {
            d.reset_fences(&[self.in_flight])?;
            d.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            let begin = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            d.begin_command_buffer(cmd, &begin)?;
        }

        graph.execute(&d, cmd);

        // SAFETY: recording is complete.
        unsafe {
            d.end_command_buffer(cmd)?;

            let wait = [self.acquired];
            let signal = [self.rendered];
            let stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let buffers = [cmd];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&wait)
                .wait_dst_stage_mask(&stages)
                .command_buffers(&buffers)
                .signal_semaphores(&signal);
            d.queue_submit(self.queue, &[submit], self.in_flight)?;

            let swapchains = [self.swapchain];
            let indices = [index];
            let present = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal)
                .swapchains(&swapchains)
                .image_indices(&indices);
            match self.swapchain_loader.queue_present(self.queue, &present) {
                Ok(_) => {}
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR | vk::Result::SUBOPTIMAL_KHR) => {
                    self.recreate()?;
                }
                Err(e) => return Err(RenderError::Vulkan(e)),
            }
        }
        Ok(())
    }

    /// Rebuild the swapchain after a resize or an out-of-date present.
    ///
    /// # Errors
    /// [`RenderError`] on Vulkan failure.
    pub fn recreate(&mut self) -> Result<(), RenderError> {
        // SAFETY: nothing may be in flight while swapchain resources are
        // destroyed — this is the use-after-free §7.3 says compiles fine.
        unsafe {
            let _ = self.device.device_wait_idle();
        }

        // SAFETY: the surface and physical device are live.
        let capabilities = unsafe {
            self.surface_loader
                .get_physical_device_surface_capabilities(self.physical, self.surface)
        }?;
        let extent = capabilities.current_extent;
        // Minimised: nothing to do, and a zero-extent swapchain is invalid.
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }

        let old = self.swapchain;
        // SAFETY: views belong to this device and nothing is in flight.
        unsafe {
            for view in self.views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
        }

        let (format, extent, swapchain, images, views) = create_swapchain(
            &self.device,
            &self.surface_loader,
            &self.swapchain_loader,
            self.physical,
            self.surface,
            extent.width,
            extent.height,
            old,
        )?;
        // SAFETY: the new swapchain was created with `old` as its predecessor,
        // so retiring it now is legal.
        unsafe {
            self.swapchain_loader.destroy_swapchain(old, None);
        }

        self.format = format;
        self.extent = extent;
        self.swapchain = swapchain;
        self.images = images;
        self.views = views;

        // Depth must match the new size.
        if let Some(allocator) = self.allocator.as_mut() {
            // SAFETY: not in flight.
            unsafe {
                self.device.destroy_image_view(self.depth_view, None);
                self.device.destroy_image(self.depth, None);
            }
            if let Some(allocation) = self.depth_alloc.take() {
                let _ = allocator.free(allocation);
            }
            let (depth, depth_alloc, depth_view) =
                create_depth(&self.device, allocator, extent.width, extent.height)?;
            self.depth = depth;
            self.depth_alloc = Some(depth_alloc);
            self.depth_view = depth_view;
        }
        Ok(())
    }

    #[must_use]
    pub fn extent(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        // SAFETY: idle first, then destroy in reverse creation order.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_semaphore(self.acquired, None);
            self.device.destroy_semaphore(self.rendered, None);
            self.device.destroy_fence(self.in_flight, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_cache(self.pipeline_cache, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            for view in self.views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
            self.device.destroy_image_view(self.depth_view, None);

            if let Some(allocator) = self.allocator.as_mut() {
                for allocation in [
                    self.depth_alloc.take(),
                    self.vertices_alloc.take(),
                    self.objects_alloc.take(),
                ]
                .into_iter()
                .flatten()
                {
                    let _ = allocator.free(allocation);
                }
            }
            self.device.destroy_image(self.depth, None);
            self.device.destroy_buffer(self.vertices, None);
            self.device.destroy_buffer(self.objects, None);
            self.allocator = None;

            self.swapchain_loader.destroy_swapchain(self.swapchain, None);
            self.surface_loader.destroy_surface(self.surface, None);
        }
    }
}

type SwapchainParts = (
    vk::Format,
    vk::Extent2D,
    vk::SwapchainKHR,
    Vec<vk::Image>,
    Vec<vk::ImageView>,
);

#[allow(clippy::too_many_arguments)]
fn create_swapchain(
    device: &ash::Device,
    surface_loader: &ash::khr::surface::Instance,
    swapchain_loader: &ash::khr::swapchain::Device,
    physical: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
    width: u32,
    height: u32,
    old: vk::SwapchainKHR,
) -> Result<SwapchainParts, RenderError> {
    // SAFETY: `physical` and `surface` are live.
    let capabilities =
        unsafe { surface_loader.get_physical_device_surface_capabilities(physical, surface) }?;
    // SAFETY: as above.
    let formats = unsafe { surface_loader.get_physical_device_surface_formats(physical, surface) }?;

    // UNORM rather than SRGB: the shader already writes linear colour, and an
    // SRGB target would apply the transfer function a second time. Queried and
    // matched rather than hardcoded (Vulkan doc §14).
    let surface_format = formats
        .iter()
        .find(|f| {
            matches!(
                f.format,
                vk::Format::B8G8R8A8_UNORM | vk::Format::R8G8B8A8_UNORM
            ) && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .copied()
        .unwrap_or_else(|| formats[0]);

    let extent = if capabilities.current_extent.width == u32::MAX {
        vk::Extent2D {
            width: width.clamp(
                capabilities.min_image_extent.width,
                capabilities.max_image_extent.width,
            ),
            height: height.clamp(
                capabilities.min_image_extent.height,
                capabilities.max_image_extent.height,
            ),
        }
    } else {
        capabilities.current_extent
    };

    let mut image_count = capabilities.min_image_count + 1;
    if capabilities.max_image_count > 0 && image_count > capabilities.max_image_count {
        image_count = capabilities.max_image_count;
    }

    let info = vk::SwapchainCreateInfoKHR::default()
        .surface(surface)
        .min_image_count(image_count)
        .image_format(surface_format.format)
        .image_color_space(surface_format.color_space)
        .image_extent(extent)
        .image_array_layers(1)
        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(capabilities.current_transform)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        // FIFO is the only mode guaranteed present, and vsync is right for a
        // viewer: the point is to look at the scene, not to melt the GPU.
        .present_mode(vk::PresentModeKHR::FIFO)
        .clipped(true)
        .old_swapchain(old);

    // SAFETY: `info` is fully initialised and outlives the call.
    let swapchain = unsafe { swapchain_loader.create_swapchain(&info, None) }?;
    // SAFETY: the swapchain was just created.
    let images = unsafe { swapchain_loader.get_swapchain_images(swapchain) }?;

    let mut views = Vec::with_capacity(images.len());
    for image in &images {
        views.push(create_view(
            device,
            *image,
            surface_format.format,
            vk::ImageAspectFlags::COLOR,
        )?);
    }

    Ok((surface_format.format, extent, swapchain, images, views))
}

fn create_depth(
    device: &ash::Device,
    allocator: &mut Allocator,
    width: u32,
    height: u32,
) -> Result<(vk::Image, Allocation, vk::ImageView), RenderError> {
    let (depth, alloc) = create_image(
        device,
        allocator,
        width,
        height,
        DEPTH_FORMAT,
        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        "loom.viewer_depth",
    )?;
    let view = create_view(device, depth, DEPTH_FORMAT, vk::ImageAspectFlags::DEPTH)?;
    Ok((depth, alloc, view))
}
