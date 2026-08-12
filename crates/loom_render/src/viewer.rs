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
    Camera, DEPTH_FORMAT, MeshRange, Object, ObjectData, RenderError, batch_by_mesh,
    begin_rendering, combine, create_address_buffer, create_image, create_index_buffer,
    create_pipeline, create_view, pack_objects, pipeline_cache_path, set_viewport,
    view_projection, write_slice,
};
use loom_render_graph::{Access, RenderGraph};
use crate::{Device, Instance};
use gpu_allocator::vulkan::{Allocation, Allocator, AllocatorCreateDesc};

/// Objects the buffer starts out able to hold. Not a ceiling — see
/// [`Viewer::reserve_objects`], which grows it. Chosen so a blockout never
/// reallocates and a big scene pays once.
const INITIAL_OBJECTS: usize = 4096;

/// A window's swapchain and everything needed to draw into it.
pub struct Viewer {
    device: ash::Device,
    /// Kept rather than dropped after construction, because resizing rebuilds
    /// images and an unnamed one is a hex handle in the next validation message.
    names: DebugNames,
    /// Whether this device can trace rays; see `Device::supports_raytracing`.
    raytracing: bool,
    raytracer: Option<crate::raytrace::Raytracer>,
    rt_positions: Option<(vk::Buffer, Allocation, vk::DeviceAddress)>,
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

    /// The anti-aliasing pass and the image the scene is drawn into first, or
    /// `None` when `LOOM_CMAA2` is unset — the default, and then the scene is
    /// drawn straight into the swapchain image exactly as before.
    ///
    /// **The window needs this as much as the offscreen path does**, and more:
    /// it is where the human looks. Grass rendered offscreen and not in the
    /// window for two slices, which is the defect this mirrors deliberately.
    aa: Option<(crate::cmaa2::Cmaa2, vk::Image, vk::ImageView, Allocation)>,

    vertices: vk::Buffer,
    vertices_alloc: Option<Allocation>,
    vertex_address: vk::DeviceAddress,
    indices: vk::Buffer,
    indices_alloc: Option<Allocation>,
    ranges: Vec<MeshRange>,
    unpack: crate::renderer::UnpackParams,
    materials: crate::material::Materials,
    particle_buffer: vk::Buffer,
    particle_alloc: Option<Allocation>,
    particle_address: vk::DeviceAddress,
    max_particles: usize,
    particle_pipeline: vk::Pipeline,
    grass_buffer: vk::Buffer,
    grass_alloc: Option<Allocation>,
    grass_address: vk::DeviceAddress,
    grass_pipeline: vk::Pipeline,
    /// Blades currently in the buffer. Uploaded on scene load, expanded every
    /// frame in the vertex shader — see [`Viewer::set_grass`].
    grass_count: u32,
    objects: vk::Buffer,
    environment_buffer: vk::Buffer,
    environment_alloc: Option<gpu_allocator::vulkan::Allocation>,
    environment_address: vk::DeviceAddress,
    /// What the next frame draws with. Set from the scene each frame.
    pub environment: crate::renderer::EnvironmentData,
    objects_alloc: Option<Allocation>,
    object_address: vk::DeviceAddress,
    /// How many objects the buffer can currently hold; grown on demand.
    object_capacity: usize,
    /// The size the window last reported. Needed because a compositor may
    /// decline to state one (Wayland's `0xFFFFFFFF`), and then this is the
    /// only real answer available.
    requested: vk::Extent2D,

    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    /// Fills the frame before the scene draws over it.
    sky_pipeline: vk::Pipeline,
    pipeline_cache: vk::PipelineCache,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    /// Signalled when the presentation engine hands us an image.
    acquired: vk::Semaphore,
    /// Signalled when rendering finishes, waited on by present — **one per
    /// swapchain image**.
    ///
    /// A single shared semaphore is the classic version of this bug: the
    /// presentation engine may still be waiting on it for the previous image
    /// when the next submit signals it again. Validation says so plainly
    /// ("is being signaled by VkQueue, but it may still be in use by
    /// VkSwapchainKHR"), and on this box it aborts inside the driver at
    /// teardown roughly one close in three.
    rendered: Vec<vk::Semaphore>,
    in_flight: vk::Fence,
    physical: vk::PhysicalDevice,
}

impl Viewer {
    /// Build a viewer for an existing surface.
    ///
    /// # Errors
    /// [`RenderError`] if any Vulkan object or allocation fails.
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instance: &Instance,
        device: &Device,
        surface: vk::SurfaceKHR,
        width: u32,
        height: u32,
        meshes: &[loom_asset::Mesh],
        textures: &[loom_asset::Texture],
        materials: &[crate::material::MaterialData],
    ) -> Result<Self, RenderError> {
        let raw = device.handle().clone();
        let raytracing = device.supports_raytracing();
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

        let (combined_vertices, combined_indices, ranges, unpack) = combine(meshes);
        // Geometry the acceleration structures are built from must say so at
        // creation. Conditional because asking for this bit without the
        // extension enabled is itself a validation error.
        let as_input = if raytracing {
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
        } else {
            vk::BufferUsageFlags::empty()
        };
        let (vertices, vertices_alloc, vertex_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (std::mem::size_of_val(combined_vertices.as_slice()) as u64).max(4),
            "loom.mesh_vertices",
            as_input,
        )?;
        write_slice(&vertices_alloc, &combined_vertices)?;

        let (indices, indices_alloc, index_address) = create_index_buffer(
            &raw,
            &mut allocator,
            (std::mem::size_of_val(combined_indices.as_slice()) as u64).max(4),
            "loom.mesh_indices",
            as_input | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        )?;
        write_slice(&indices_alloc, &combined_indices)?;

        let (objects, objects_alloc, object_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (INITIAL_OBJECTS * size_of::<ObjectData>()) as u64,
            "loom.object_data",
            vk::BufferUsageFlags::empty(),
        )?;
        let (environment_buffer, environment_alloc, environment_address) = create_address_buffer(
            &raw,
            &mut allocator,
            size_of::<crate::renderer::EnvironmentData>() as u64,
            "loom.environment",
            vk::BufferUsageFlags::empty(),
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

        // Same shader as the headless path, so the same descriptor must exist:
        // the fragment stage declares the TLAS unconditionally, and a layout
        // without it is VUID-VkGraphicsPipelineCreateInfo-layout-07988.
        let mut raytracer = if raytracing {
            Some(crate::raytrace::Raytracer::new(
                instance.handle(),
                &raw,
                &mut allocator,
            )?)
        } else {
            None
        };

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(device.queue_family())
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the family index came from this device.
        let command_pool = unsafe { raw.create_command_pool(&pool_info, None) }?;

        // Same ordering constraint as the offscreen path: after the command
        // pool because uploading is queue work, before the pipeline because the
        // layout needs its descriptor set layout.
        let materials = crate::material::Materials::new(
            &raw,
            &mut allocator,
            textures,
            materials,
            crate::raytrace::Submit { pool: command_pool, queue: device.queue() },
        )?;

        let cache_path = pipeline_cache_path(instance, device);
        // The pipeline is built for the *swapchain's* format, which is usually
        // BGRA rather than the offscreen path's RGBA. Dynamic rendering bakes
        // the attachment format into the pipeline, so this cannot be shared.
        let (pipeline_layout, pipeline, pipeline_cache) =
            create_pipeline(
                &raw,
                cache_path.as_deref(),
                format,
                raytracer.as_ref().map(crate::raytrace::Raytracer::descriptor_layout),
                materials.descriptor_layout(),
                ash::vk::SampleCountFlags::TYPE_1,
            )?;
        let sky_pipeline =
            crate::renderer::create_sky_pipeline(
                &raw, pipeline_layout, pipeline_cache, format,
                ash::vk::SampleCountFlags::TYPE_1,
            )?;
        let particle_pipeline = crate::renderer::create_particle_pipeline(
            &raw,
            pipeline_layout,
            pipeline_cache,
            format,
            ash::vk::SampleCountFlags::TYPE_1,
        )?;
        // **One sample, unlike the offscreen path's four.** A pipeline's
        // rasterisation sample count must match the attachment it draws into,
        // and this one draws straight into the swapchain image.
        let grass_pipeline = crate::renderer::create_grass_pipeline(
            &raw,
            pipeline_layout,
            pipeline_cache,
            format,
            ash::vk::SampleCountFlags::TYPE_1,
        )?;

        // Mirrors the offscreen path's ceiling. Sized once so a frame never
        // allocates, which in a live viewer would show up as a hitch.
        const MAX_PARTICLES: usize = 65536;
        let (particle_buffer, particle_alloc, particle_address) =
            crate::renderer::create_address_buffer(
                &raw,
                &mut allocator,
                (MAX_PARTICLES * size_of::<crate::renderer::ParticleInstance>()) as u64,
                "loom.viewer_particles",
                vk::BufferUsageFlags::empty(),
            )?;
        // Same ceiling as `renderer.rs`, and the same reason: past it the tail
        // is dropped rather than the buffer growing mid-frame.
        const MAX_BLADES: usize = 262_144;
        let (grass_buffer, grass_alloc, grass_address) = crate::renderer::create_address_buffer(
            &raw,
            &mut allocator,
            (MAX_BLADES * size_of::<crate::renderer::GrassBlade>()) as u64,
            "loom.viewer_grass",
            vk::BufferUsageFlags::empty(),
        )?;

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
        let (acquired, in_flight) = unsafe {
            (
                raw.create_semaphore(&semaphore_info, None)?,
                raw.create_fence(&fence_info, None)?,
            )
        };
        let mut rendered = Vec::with_capacity(images.len());
        for _ in 0..images.len() {
            // SAFETY: a default semaphore create-info is always valid.
            rendered.push(unsafe { raw.create_semaphore(&semaphore_info, None) }?);
        }

        // The AA pass reads the scene and writes the swapchain image, so what
        // it needs here is a *source* to render the scene into. The swapchain
        // image cannot be that source: its usage is COLOR_ATTACHMENT only, and
        // asking a presentable image for SAMPLED is not portable.
        let aa = if crate::cmaa2::requested() {
            Some(create_aa_target(
                &raw,
                &mut allocator,
                &names,
                pipeline_cache,
                format,
                extent,
            )?)
        } else {
            None
        };

        names.set(pipeline, "loom.viewer_pipeline");
        names.set(grass_pipeline, "loom.viewer_grass_pipeline");
        names.set(grass_buffer, "loom.viewer_grass");
        names.set(depth, "loom.viewer_depth");
        names.set(acquired, "loom.sem_image_acquired");
        for semaphore in &rendered {
            names.set(*semaphore, "loom.sem_render_finished");
        }

        let mut rt_positions = None;
        if let Some(rt) = raytracer.as_mut() {
            // Unpacked positions: the raster path's vertices are quantised and
            // an acceleration structure cannot undo that. See `renderer.rs`.
            let positions: Vec<[f32; 3]> = meshes
                .iter()
                .flat_map(|m| m.vertices.iter())
                .map(|v| [v.position[0], v.position[1], v.position[2]])
                .collect();
            let (buffer, allocation, address) = create_address_buffer(
                &raw,
                &mut allocator,
                (std::mem::size_of_val(positions.as_slice()) as u64).max(4),
                "loom.rt_positions",
                vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
            )?;
            write_slice(&allocation, &positions)?;
            rt.build_meshes(
                &mut allocator,
                crate::raytrace::Submit { pool: command_pool, queue: device.queue() },
                address,
                index_address,
                &ranges,
                u32::try_from(combined_vertices.len()).unwrap_or(0),
            )?;
            rt_positions = Some((buffer, allocation, address));
        }

        Ok(Self {
            names,
            raytracing,
            raytracer,
            materials,
            particle_buffer,
            particle_alloc: Some(particle_alloc),
            particle_address,
            max_particles: MAX_PARTICLES,
            particle_pipeline,
            grass_buffer,
            grass_alloc: Some(grass_alloc),
            grass_address,
            grass_pipeline,
            grass_count: 0,
            rt_positions,
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
            aa,
            vertices,
            vertices_alloc: Some(vertices_alloc),
            vertex_address,
            indices,
            indices_alloc: Some(indices_alloc),
            ranges,
            unpack,
            objects,
            objects_alloc: Some(objects_alloc),
            object_address,
            environment_buffer,
            environment_alloc: Some(environment_alloc),
            environment_address,
            environment: crate::renderer::EnvironmentData::default(),
            object_capacity: INITIAL_OBJECTS,
            requested: vk::Extent2D { width, height },
            pipeline_layout,
            pipeline,
            sky_pipeline,
            pipeline_cache,
            command_pool,
            command_buffer,
            acquired,
            rendered,
            in_flight,
            physical: device.physical(),
        })
    }

    /// Hand the viewer this frame's grass.
    ///
    /// Mirrors [`crate::Renderer::set_grass`] exactly, including the ceiling:
    /// past capacity the tail is dropped rather than the buffer growing. Called
    /// on scene load and on every reload, not per frame — a blade is a pure
    /// function of its coordinates, and what moves is the Bézier expansion and
    /// the wind bend, both of which happen in the vertex shader.
    ///
    /// How many blades the buffer holds. Past this the tail is dropped, so a
    /// caller that cares — and it should, the drop is a contiguous spatial slab
    /// rather than a scatter — can say so before uploading.
    #[must_use]
    pub fn grass_capacity(&self) -> usize {
        self.grass_alloc.as_ref().map_or(0, |a| {
            a.size() as usize / size_of::<crate::renderer::GrassBlade>()
        })
    }

    /// # Errors
    /// If the buffer is gone, which means the viewer is being torn down.
    pub fn set_grass(&mut self, blades: &[crate::renderer::GrassBlade]) -> Result<(), RenderError> {
        let capacity = self.grass_capacity();
        let drawn = &blades[..blades.len().min(capacity)];
        self.grass_count = u32::try_from(drawn.len()).unwrap_or(0);
        if drawn.is_empty() {
            return Ok(());
        }
        write_slice(
            self.grass_alloc
                .as_ref()
                .ok_or_else(|| RenderError::Allocator("grass buffer is gone".into()))?,
            drawn,
        )
    }

    /// Make sure the object buffer can hold `wanted` objects.
    ///
    /// Doubling, so a scene that grows steadily reallocates a handful of times
    /// rather than every frame. Each object is 144 bytes: the initial 4096 is
    /// 590 KB and even a hundred thousand is 14 MB, which is why growing is the
    /// right answer and a hard ceiling was not.
    ///
    /// # Errors
    /// [`RenderError`] if the device will not idle or the buffer cannot be
    /// reallocated.
    pub fn reserve_objects(&mut self, wanted: usize) -> Result<(), RenderError> {
        if wanted <= self.object_capacity {
            return Ok(());
        }
        let mut capacity = self.object_capacity.max(1);
        while capacity < wanted {
            capacity = capacity.saturating_mul(2);
        }

        let Some(allocator) = self.allocator.as_mut() else {
            return Ok(());
        };
        // SAFETY: the frame in flight may still be reading the old buffer.
        unsafe { self.device.device_wait_idle() }?;

        // **Build before destroying.** Freeing first leaves `self.objects`
        // holding a destroyed handle if the re-create fails, which `Drop` then
        // destroys again — the exact double-free fixed in `set_meshes`, which
        // I reintroduced here while adding buffer growth. A transient
        // allocation spike is the cheaper mistake.
        let (objects, objects_alloc, object_address) = create_address_buffer(
            &self.device,
            allocator,
            (capacity * size_of::<ObjectData>()) as u64,
            "loom.object_data",
            vk::BufferUsageFlags::empty(),
        )?;
        // Everything fallible is done; retire the old buffer now.
        if let Some(old) = self.objects_alloc.take() {
            let _ = allocator.free(old);
        }
        // SAFETY: the device is idle and the handle is ours.
        unsafe { self.device.destroy_buffer(self.objects, None) };

        self.objects = objects;
        self.objects_alloc = Some(objects_alloc);
        self.object_address = object_address;
        self.object_capacity = capacity;
        Ok(())
    }

    /// Replace the geometry this viewer draws from.
    ///
    /// This is what lets a window follow a file that is still being written —
    /// by the human in the editor or by the agent through the CLI. Moving a
    /// node does **not** come through here; only a changed mesh *set* does,
    /// because reallocating buffers per slider frame would be absurd.
    ///
    /// Idles the device first. The frame in flight is reading the buffers about
    /// to be freed, and per-buffer fences are not worth it for something that
    /// happens when a file changes rather than every frame.
    ///
    /// # Errors
    /// [`RenderError`] if the device will not idle or the buffers cannot be
    /// reallocated.
    pub fn set_meshes(&mut self, meshes: &[loom_asset::Mesh]) -> Result<(), RenderError> {
        let raytracing = self.raytracing;
        let Some(allocator) = self.allocator.as_mut() else {
            return Ok(());
        };
        // SAFETY: idling is what makes freeing these legal.
        unsafe { self.device.device_wait_idle() }?;
        // Idle is not enough on its own: the command buffer still *references*
        // the old buffers until it is reset, and validation rightly refuses a
        // `vkDestroyBuffer` on something a recorded command buffer holds
        // (VUID-vkDestroyBuffer-buffer-00922).
        //
        // SAFETY: the device is idle, so this command buffer is not executing.
        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
        }?;

        let (combined_vertices, combined_indices, ranges, unpack) = combine(meshes);
        // Geometry the acceleration structures are built from must say so at
        // creation. Conditional because asking for this bit without the
        // extension enabled is itself a validation error.
        let as_input = if raytracing {
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
        } else {
            vk::BufferUsageFlags::empty()
        };

        // **Build the new buffers before destroying the old ones.** This used
        // to free first to avoid holding both peaks at once — but any `?`
        // between the free and the re-create left `self.vertices` /
        // `self.indices` holding destroyed handles, which `Drop` then destroys
        // a second time. Correctness over a transient allocation spike: a
        // failed re-upload now leaves the viewer exactly as it was.
        let (vertices, vertices_alloc, vertex_address) = create_address_buffer(
            &self.device,
            allocator,
            (std::mem::size_of_val(combined_vertices.as_slice()) as u64).max(4),
            "loom.mesh_vertices",
            as_input,
        )?;
        write_slice(&vertices_alloc, &combined_vertices)?;

        let (indices, indices_alloc, _) = create_index_buffer(
            &self.device,
            allocator,
            (std::mem::size_of_val(combined_indices.as_slice()) as u64).max(4),
            "loom.mesh_indices",
            as_input | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        )?;
        write_slice(&indices_alloc, &combined_indices)?;

        // Everything fallible is done. Now retire the old buffers.
        for allocation in [self.vertices_alloc.take(), self.indices_alloc.take()]
            .into_iter()
            .flatten()
        {
            let _ = allocator.free(allocation);
        }
        // SAFETY: the device is idle and these handles are ours.
        unsafe {
            self.device.destroy_buffer(self.vertices, None);
            self.device.destroy_buffer(self.indices, None);
        }

        self.vertices = vertices;
        self.vertices_alloc = Some(vertices_alloc);
        self.vertex_address = vertex_address;
        self.indices = indices;
        self.indices_alloc = Some(indices_alloc);
        self.ranges = ranges;
        self.unpack = unpack;
        Ok(())
    }

    /// Draw one frame and present it.
    ///
    /// # Errors
    /// [`RenderError`] on Vulkan failure. A resize is handled internally and
    /// is not an error.
    #[allow(clippy::too_many_lines)]
    pub fn draw(&mut self, objects: &[Object], camera: &Camera) -> Result<(), RenderError> {
        self.draw_with_ui(objects, &[], camera, None, |_| {})
    }

    /// Draw a frame with an optional UI layer over it.
    ///
    /// The UI records into the **same** dynamic-rendering pass as the scene, so
    /// there is no second pass and no render-pass object (never-do #1). It is
    /// recorded after the geometry and before the present transition, which is
    /// what puts panels on top.
    ///
    /// # Errors
    /// [`RenderError`] on Vulkan failure.
    #[allow(clippy::too_many_lines)]
    pub fn draw_with_ui(
        &mut self,
        objects: &[Object],
        particles: &[crate::renderer::ParticleInstance],
        camera: &Camera,
        ui: Option<(&mut crate::Ui, &winit::window::Window)>,
        build: impl FnMut(&mut egui::Ui),
    ) -> Result<(), RenderError> {
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
        let mut sorted: Vec<Object> = objects.to_vec();
        sorted.sort_by_key(|o| o.mesh);
        // Grow rather than refuse. An agent that generates a five-thousand
        // node scene should see it, not an error about a buffer size it has no
        // way to know about. `write_slice` still guards the memory.
        self.reserve_objects(objects.len())?;

        let view_proj = view_projection(camera, aspect);
        let mut object_data = pack_objects(&sorted, view_proj, &self.unpack);
        // The reserved slot the particle vertex shader reads its
        // view-projection from. See the note on `Push::object_offset`.
        let particle_slot = u32::try_from(object_data.len()).unwrap_or(0);
        object_data.push(crate::renderer::view_projection_slot(view_proj));

        // Sorted back to front, tiebroken by original index so the order is
        // total. Identical to the offscreen path, and for the same reason:
        // these blend, so what is drawn depends on the order it is drawn in.
        let mut ordered: Vec<(usize, crate::renderer::ParticleInstance)> =
            particles.iter().copied().enumerate().collect();
        let eye = camera.eye;
        ordered.sort_by(|(ai, a), (bi, b)| {
            let d = |p: &crate::renderer::ParticleInstance| {
                let (dx, dy, dz) = (
                    p.position[0] - eye.x,
                    p.position[1] - eye.y,
                    p.position[2] - eye.z,
                );
                dz.mul_add(dz, dx.mul_add(dx, dy * dy))
            };
            d(b).partial_cmp(&d(a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(ai.cmp(bi))
        });
        let drawn: Vec<crate::renderer::ParticleInstance> = ordered
            .into_iter()
            .take(self.max_particles)
            .map(|(_, p)| p)
            .collect();
        let particle_count = u32::try_from(drawn.len()).unwrap_or(0);
        if !drawn.is_empty() {
            crate::renderer::write_slice(
                self.particle_alloc
                    .as_ref()
                    .ok_or_else(|| RenderError::Allocator("particle buffer is gone".into()))?,
                &drawn,
            )?;
        }
        // The TLAS follows the objects; the BLAS holding the triangles does not
        // move, which is what keeps this affordable per frame.
        if self.raytracer.is_some() {
            let pool = self.command_pool;
            let queue = self.queue;
            let mut allocator = self.allocator.take();
            let result = match (self.raytracer.as_mut(), allocator.as_mut()) {
                (Some(rt), Some(alloc)) => rt.build_instances(
                    alloc,
                    crate::raytrace::Submit { pool, queue },
                    &sorted,
                ),
                _ => Ok(()),
            };
            self.allocator = allocator;
            result?;
        }
        let batches = batch_by_mesh(&sorted);
        // **Before the upload, not after**, the same as the offscreen path. The
        // camera moved into this buffer when the push block ran out of room, and
        // it was being written a line *below* the upload — so the window drew
        // every frame with the previous frame's eye. The viewport was never
        // written at all: it sat at its 1x1 default, which the grass shader
        // reads as "every blade is a hundred pixels wide".
        self.environment.eye = camera.eye.extend(0.0).to_array();
        #[allow(clippy::cast_precision_loss)]
        {
            self.environment.viewport =
                [self.extent.width as f32, self.extent.height as f32, 0.0, 0.0];
        }
        write_slice(
            self.environment_alloc
                .as_ref()
                .ok_or_else(|| RenderError::Allocator("environment buffer missing".into()))?,
            std::slice::from_ref(&self.environment),
        )?;
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
        let (queue, pool) = (self.queue, self.command_pool);
        let (pipeline, layout) = (self.pipeline, self.pipeline_layout);
        let shadow_set = self
            .raytracer
            .as_ref()
            .filter(|rt| rt.ready())
            .map(crate::raytrace::Raytracer::descriptor_set);
        let material_set = self.materials.descriptor_set();
        let base_push = crate::renderer::Push {
            vertices: self.vertex_address,
            objects: self.object_address,
            environment: self.environment_address,
            materials: self.materials.address(),
            particles: self.particle_address,
            grass: self.grass_address,
            object_offset: 0,
            inv_view_proj: view_proj.inverse().to_cols_array(),
        };
        let sky = self.sky_pipeline;
        let particle_pipeline = self.particle_pipeline;
        let grass_pipeline = self.grass_pipeline;
        let grass_count = self.grass_count;
        let index_buffer = self.indices;
        let draws: Vec<(MeshRange, u32, u32)> = batches
            .iter()
            .filter_map(|(mesh, first, count)| {
                self.ranges.get(*mesh as usize).map(|r| (*r, *first, *count))
            })
            .collect();
        let depth_image = self.depth;
        let depth_id = graph.import("loom.viewer_depth", depth_image);

        // With the AA pass on, the scene is drawn into its own image and the
        // pass writes the swapchain; without it, straight into the swapchain as
        // before. Everything downstream — the UI, the present transition — is
        // unchanged either way.
        //
        // **The UI is inside the forward pass**, so with AA on it is filtered
        // along with the scene. CMAA2 is conservative enough that egui's own
        // anti-aliased text is left almost entirely alone, and moving the UI
        // into a third pass to avoid it would buy a second rendering block for
        // a difference nobody can see.
        let (scene_view, scene_id) = match self.aa.as_ref() {
            Some((_, image, aa_view, _)) => {
                (*aa_view, graph.import("loom.viewer_scene", *image))
            }
            None => (view, target),
        };

        graph.pass(
            "forward",
            &[(scene_id, Access::ColorWrite), (depth_id, Access::DepthWrite)],
            move |d, cmd| {
                // SAFETY: the graph transitioned both attachments already.
                unsafe {
                    // The viewer renders at one sample; MSAA is the offscreen path for now.
                    begin_rendering(d, cmd, scene_view, depth_view, None, extent.width, extent.height);
                    set_viewport(d, cmd, extent.width, extent.height);
                    crate::renderer::draw_sky(d, cmd, sky, layout, &base_push);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    // Bound once for the pass, never per draw (never-do #2).
                    if let Some(set) = shadow_set {
                        d.cmd_bind_descriptor_sets(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            layout,
                            0,
                            &[set],
                            &[],
                        );
                    }
                    // Set 1: the scene's textures.
                    d.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        layout,
                        1,
                        &[material_set],
                        &[],
                    );
                    set_viewport(d, cmd, extent.width, extent.height);
                    d.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);
                    for (range, first_instance, instances) in draws {
                        let push = crate::renderer::Push {
                            object_offset: first_instance,
                            ..base_push
                        };
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
                        d.cmd_draw_indexed(
                            cmd,
                            range.index_count(),
                            instances,
                            range.first_index(),
                            0,
                            0,
                        );
                    }

                    // Grass after the meshes and before the particles, exactly
                    // as the offscreen path orders it: it is opaque and
                    // depth-written, so smoke has to blend over finished
                    // blades. 42 vertices per blade (`GRASS_VERTS` in
                    // `scene.slang`), no vertex buffer.
                    if grass_count > 0 {
                        d.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            grass_pipeline,
                        );
                        let push = crate::renderer::Push {
                            object_offset: particle_slot,
                            ..base_push
                        };
                        d.cmd_push_constants(
                            cmd,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            push.bytes(),
                        );
                        d.cmd_draw(cmd, grass_count * 42, 1, 0, 0);
                    }

                    // Particles last, over finished opaque geometry.
                    if particle_count > 0 {
                        d.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            particle_pipeline,
                        );
                        let push = crate::renderer::Push {
                            object_offset: particle_slot,
                            ..base_push
                        };
                        d.cmd_push_constants(
                            cmd,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            push.bytes(),
                        );
                        d.cmd_draw(cmd, particle_count * 6, 1, 0, 0);
                    }
                    // The UI goes inside the same rendering pass, after the
                    // geometry, so panels draw over the scene without a second
                    // pass or a render-pass object.
                    if let Some((ui, window)) = ui {
                        let _ = ui.draw(window, cmd, extent, queue, pool, build);
                    }
                    d.cmd_end_rendering(cmd);
                }
            },
        );

        // The anti-aliasing passes: edge detection into the mask image, then
        // the shape filter reading the finished scene and the mask and writing
        // the swapchain image. Every transition is the graph's (never-do #4).
        if let Some((pass, _, _, _)) = self.aa.as_ref() {
            let edges_id = graph.import("loom.aa_edges", pass.edges_image());
            graph.pass(
                "cmaa2_edges",
                &[(scene_id, Access::ShaderRead), (edges_id, Access::ColorWrite)],
                move |d, cmd| {
                    // SAFETY: the graph has put the scene image in
                    // SHADER_READ_ONLY_OPTIMAL and the mask in
                    // COLOR_ATTACHMENT_OPTIMAL.
                    unsafe { pass.record_edges(d, cmd, extent.width, extent.height) };
                },
            );
            graph.pass(
                "cmaa2",
                &[
                    (scene_id, Access::ShaderRead),
                    (edges_id, Access::ShaderRead),
                    (target, Access::ColorWrite),
                ],
                move |d, cmd| {
                    // SAFETY: as above, and the swapchain image is in
                    // COLOR_ATTACHMENT_OPTIMAL.
                    unsafe { pass.record(d, cmd, view, extent.width, extent.height) };
                },
            );
        }

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
            // The semaphore belonging to *this* image, so a present still
            // pending on another image is never signalled a second time.
            let signal = [self.rendered[index as usize]];
            // ALL_COMMANDS, not COLOR_ATTACHMENT_OUTPUT. The first thing the
            // command buffer does is a layout transition on the acquired
            // image, and a barrier executes earlier in the pipeline than the
            // colour stage — so waiting only at COLOR_ATTACHMENT_OUTPUT lets
            // that write race the acquire, which sync validation reports as
            // SYNC-HAZARD-WRITE-AFTER-READ.
            //
            // `ponytail:` the tighter fix is to give the graph's first barrier
            // a matching srcStageMask, but barriers belong to the graph
            // (never-do #4) and this is one wait per frame. Revisit if the
            // stall ever shows up in a profile.
            let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
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
    /// Rebuild the swapchain for a window that is now `width` x `height`.
    ///
    /// # Errors
    /// [`RenderError`] on Vulkan failure.
    pub fn recreate_sized(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        if width > 0 && height > 0 {
            self.requested = vk::Extent2D { width, height };
        }
        self.recreate()
    }

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
        // `current_extent` is the compositor telling us the size — except on
        // Wayland, where it is defined to be `0xFFFFFFFF` meaning "you choose".
        // Taking it raw asked for a swapchain of about four billion pixels a
        // side. Fall back to the size winit last reported, which is the only
        // party that actually knows.
        let extent = if capabilities.current_extent.width == u32::MAX {
            self.requested
        } else {
            capabilities.current_extent
        };
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
        // The recreated swapchain can hand back a different number of images,
        // and each needs its own render-finished semaphore.
        while self.rendered.len() < images.len() {
            let info = vk::SemaphoreCreateInfo::default();
            // SAFETY: nothing is in flight — the device was idled above.
            self.rendered
                .push(unsafe { self.device.create_semaphore(&info, None) }?);
        }
        while self.rendered.len() > images.len() {
            if let Some(extra) = self.rendered.pop() {
                // SAFETY: as above, and this handle is ours.
                unsafe { self.device.destroy_semaphore(extra, None) };
            }
        }

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

            // The AA source is a full-resolution image like the depth buffer,
            // so it resizes with the window. The pass itself — pipeline,
            // sampler, descriptor set — survives; only the image it points at
            // is rebuilt, and the descriptor is repointed at the new view.
            if let Some((mut pass, image, view, allocation)) = self.aa.take() {
                // SAFETY: the device was idled at the top of this function.
                unsafe {
                    self.device.destroy_image_view(view, None);
                    self.device.destroy_image(image, None);
                }
                let _ = allocator.free(allocation);
                let (image, allocation) = create_image(
                    &self.device,
                    allocator,
                    extent.width,
                    extent.height,
                    self.format,
                    vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
                    1,
                    vk::SampleCountFlags::TYPE_1,
                    "loom.viewer_scene",
                )?;
                let view = create_view(
                    &self.device,
                    image,
                    self.format,
                    vk::ImageAspectFlags::COLOR,
                )?;
                // SAFETY: the device is idle, so no command buffer references
                // the sets being rewritten or the edge image being rebuilt.
                unsafe {
                    pass.rebind(
                        &self.device,
                        allocator,
                        &self.names,
                        view,
                        extent.width,
                        extent.height,
                    )?;
                }
                self.aa = Some((pass, image, view, allocation));
            }
        }
        Ok(())
    }

    /// The swapchain's colour format, for building a matching UI pipeline.
    #[must_use]
    pub fn color_format(&self) -> vk::Format {
        self.format
    }

    #[must_use]
    pub fn extent(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        // SAFETY: idle first, then destroy in an order that respects what is
        // still using what.
        //
        // **The swapchain goes before the semaphores.** `rendered` is the
        // semaphore `vkQueuePresentKHR` waits on, and `vkDeviceWaitIdle` says
        // nothing about the *presentation engine* — it only drains the queues.
        // Destroying a semaphore a pending present still waits on is
        // VUID-vkDestroySemaphore-semaphore-01137, and on this box it aborts
        // inside the driver perhaps one close in three. Once the swapchain is
        // gone there is no outstanding present to wait on anything.
        unsafe {
            let _ = self.device.device_wait_idle();
            if let (Some(rt), Some(allocator)) =
                (self.raytracer.as_mut(), self.allocator.as_mut())
            {
                rt.destroy(allocator);
            }
            // After the idle wait, for the same reason: these are images the
            // fragment shader was sampling one frame ago.
            if let Some(allocator) = self.allocator.as_mut() {
                self.materials.destroy(allocator);
            }
            if let (Some((mut pass, image, view, allocation)), Some(allocator)) =
                (self.aa.take(), self.allocator.as_mut())
            {
                pass.destroy(&self.device, allocator);
                self.device.destroy_image_view(view, None);
                self.device.destroy_image(image, None);
                let _ = allocator.free(allocation);
            }
            self.device.destroy_pipeline(self.particle_pipeline, None);
            self.device.destroy_buffer(self.particle_buffer, None);
            if let (Some(allocation), Some(allocator)) =
                (self.particle_alloc.take(), self.allocator.as_mut())
            {
                let _ = allocator.free(allocation);
            }
            self.device.destroy_pipeline(self.grass_pipeline, None);
            self.device.destroy_buffer(self.grass_buffer, None);
            if let (Some(allocation), Some(allocator)) =
                (self.grass_alloc.take(), self.allocator.as_mut())
            {
                let _ = allocator.free(allocation);
            }
            if let (Some((buffer, allocation, _)), Some(allocator)) =
                (self.rt_positions.take(), self.allocator.as_mut())
            {
                let _ = allocator.free(allocation);
                self.device.destroy_buffer(buffer, None);
            }

            // Image views first: they are views *of* the swapchain's images.
            for view in self.views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
            self.swapchain_loader.destroy_swapchain(self.swapchain, None);

            // Now nothing can be waiting on these.
            self.device.destroy_semaphore(self.acquired, None);
            for semaphore in self.rendered.drain(..) {
                self.device.destroy_semaphore(semaphore, None);
            }
            self.device.destroy_fence(self.in_flight, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline(self.sky_pipeline, None);
            self.device.destroy_pipeline_cache(self.pipeline_cache, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_image_view(self.depth_view, None);

            if let Some(allocator) = self.allocator.as_mut() {
                for allocation in [
                    self.depth_alloc.take(),
                    self.vertices_alloc.take(),
                    self.indices_alloc.take(),
                    self.objects_alloc.take(),
                    self.environment_alloc.take(),
                ]
                .into_iter()
                .flatten()
                {
                    let _ = allocator.free(allocation);
                }
            }
            self.device.destroy_image(self.depth, None);
            self.device.destroy_buffer(self.vertices, None);
            self.device.destroy_buffer(self.indices, None);
            self.device.destroy_buffer(self.objects, None);
            self.device.destroy_buffer(self.environment_buffer, None);
            self.allocator = None;

            // The surface last: the swapchain was built from it.
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

    // **SRGB, and the comment that used to be here had it backwards.** It read
    // "UNORM rather than SRGB: the shader already writes linear colour, and an
    // SRGB target would apply the transfer function a second time." Writing
    // linear colour is precisely why the target has to encode: eight bits of
    // linear light displayed as if it were sRGB is about a gamma too dark.
    //
    // The offscreen path was fixed when a real texture made it obvious there.
    // This one kept the bug, so the window rendered darker than the PNG of the
    // same scene — which is the worst version of it, because the window is
    // what a human looks at and the PNG is what the agent checks.
    //
    // Queried and matched rather than hardcoded (Vulkan doc §14), falling back
    // to whatever the surface offers if no sRGB format is available.
    let surface_format = formats
        .iter()
        .find(|f| {
            matches!(
                f.format,
                vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB
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

/// The image the scene is drawn into when the AA pass is on, and the pass
/// itself, already pointed at it.
fn create_aa_target(
    device: &ash::Device,
    allocator: &mut Allocator,
    names: &DebugNames,
    cache: vk::PipelineCache,
    format: vk::Format,
    extent: vk::Extent2D,
) -> Result<(crate::cmaa2::Cmaa2, vk::Image, vk::ImageView, Allocation), RenderError> {
    let (image, allocation) = create_image(
        device,
        allocator,
        extent.width,
        extent.height,
        format,
        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
        1,
        vk::SampleCountFlags::TYPE_1,
        "loom.viewer_scene",
    )?;
    let view = create_view(device, image, format, vk::ImageAspectFlags::COLOR)?;
    // The swapchain's format, not the offscreen path's: dynamic rendering bakes
    // the attachment format into the pipeline, and this one writes the window.
    let pass = crate::cmaa2::Cmaa2::new(
        device,
        allocator,
        names,
        cache,
        format,
        view,
        extent.width,
        extent.height,
    )?;
    names.set(image, "loom.viewer_scene");
    names.set(view, "loom.viewer_scene.view");
    Ok((pass, image, view, allocation))
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
        1,
        vk::SampleCountFlags::TYPE_1,
        "loom.viewer_depth",
    )?;
    let view = create_view(device, depth, DEPTH_FORMAT, vk::ImageAspectFlags::DEPTH)?;
    Ok((depth, alloc, view))
}
