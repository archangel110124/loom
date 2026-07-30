//! Headless offscreen renderer: draw a scene to a `VkImage`, read it back, PNG.
//!
//! Brief §7.1 — this is the highest-leverage decision in the project. The agent
//! cannot look at a window, so PNG writeout lands *before* the swapchain and
//! becomes the primary render path rather than a feature bolted on later. That
//! also guarantees `render_preview` can never diverge from "the real renderer",
//! because it *is* the real renderer.
//!
//! Dynamic rendering only — no `VkRenderPass`, no `VkFramebuffer` (never-do #1).
//! All memory through `gpu-allocator` (never-do #3).

use ash::vk;
use glam::{Mat4, Vec3};
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{
    Allocation, AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc,
};

use loom_render_graph::{Access, RenderGraph, Transition};

use crate::debug_names::DebugNames;
use crate::{Device, Instance};

pub(crate) const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
pub(crate) const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

/// One object to draw: a cube with a transform and a colour.
#[derive(Debug, Clone, Copy)]
pub struct Object {
    /// World transform.
    pub model: Mat4,
    /// Linear RGB.
    pub color: [f32; 3],
}

/// What the camera looks at.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_y_degrees: f32,
}

/// Push-constant block: two device addresses, nothing else.
///
/// Must match `Push` in `scene.slang` exactly — a mismatch is garbage on screen
/// with no diagnostic (brief §7.7), so the sizes are asserted in a test.
/// Keeping only pointers here means per-object fields can grow without ever
/// running into the 128-byte push-constant limit.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Push {
    pub(crate) vertices: vk::DeviceAddress,
    pub(crate) objects: vk::DeviceAddress,
}

/// One cube vertex. 16-byte aligned members: `[f32; 3]` would place `normal`
/// at offset 12 and violate std430 for a `PhysicalStorageBuffer` block, which
/// `spirv-val` rejects outright.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Vertex {
    position: [f32; 4],
    normal: [f32; 4],
}

/// Per-object data, indexed by `SV_InstanceID`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ObjectData {
    mvp: [f32; 16],
    /// Rows of inverse-transpose(model)'s upper 3x3, padded to `vec4`.
    normal: [[f32; 4]; 3],
    color: [f32; 4],
}

/// The unit cube, expanded to 36 vertices with per-face normals.
///
/// Built on the CPU now that geometry lives in a buffer. Real meshes replace
/// this at M5; the buffer and the address plumbing do not change.
pub(crate) fn cube_vertices() -> Vec<Vertex> {
    const CORNERS: [[f32; 3]; 8] = [
        [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0],
    ];
    // 6 faces x 2 triangles, counter-clockwise seen from outside.
    const INDICES: [usize; 36] = [
        0, 2, 1, 0, 3, 2, 5, 7, 4, 5, 6, 7, 4, 3, 0, 4, 7, 3,
        1, 6, 5, 1, 2, 6, 4, 1, 5, 4, 0, 1, 3, 6, 2, 3, 7, 6,
    ];
    const NORMALS: [[f32; 3]; 6] = [
        [0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 1.0, 0.0],
    ];
    INDICES
        .iter()
        .enumerate()
        .map(|(i, &corner)| {
            let p = CORNERS[corner];
            let n = NORMALS[i / 6];
            Vertex {
                position: [p[0], p[1], p[2], 1.0],
                normal: [n[0], n[1], n[2], 0.0],
            }
        })
        .collect()
}

/// Anything that can go wrong rendering.
#[derive(Debug)]
pub enum RenderError {
    Vulkan(vk::Result),
    Allocator(String),
    /// Validation was not silent. Carries the messages.
    Validation(Vec<String>),
    Io(std::io::Error),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "vulkan error: {r:?}"),
            Self::Allocator(e) => write!(f, "allocation failed: {e}"),
            Self::Validation(m) => write!(f, "validation was not silent:\n  {}", m.join("\n  ")),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<vk::Result> for RenderError {
    fn from(r: vk::Result) -> Self {
        Self::Vulkan(r)
    }
}

/// An offscreen renderer sized to one output resolution.
pub struct Renderer {
    device: ash::Device,
    queue: vk::Queue,
    allocator: Option<Allocator>,

    width: u32,
    height: u32,

    color: vk::Image,
    color_view: vk::ImageView,
    color_alloc: Option<Allocation>,
    depth: vk::Image,
    depth_view: vk::ImageView,
    depth_alloc: Option<Allocation>,
    readback: vk::Buffer,
    readback_alloc: Option<Allocation>,
    vertices: vk::Buffer,
    vertices_alloc: Option<Allocation>,
    vertex_address: vk::DeviceAddress,
    objects: vk::Buffer,
    objects_alloc: Option<Allocation>,
    object_address: vk::DeviceAddress,
    max_objects: usize,

    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    pipeline_cache: vk::PipelineCache,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    cache_path: Option<std::path::PathBuf>,
    /// What the graph decided last frame. Exposed so a test can assert the
    /// barriers actually happened rather than trusting that they did.
    last_transitions: Vec<Transition>,
}

impl Renderer {
    /// Build every resource needed to render at `width` x `height`.
    ///
    /// # Errors
    /// [`RenderError`] if any Vulkan object or allocation fails.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        instance: &Instance,
        device: &Device,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let raw = device.handle().clone();
        // Enabled exactly when the instance enabled the extension.
        let names = DebugNames::new(instance.handle(), &raw, cfg!(debug_assertions));

        let mut allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.handle().clone(),
            device: raw.clone(),
            physical_device: device.physical(),
            debug_settings: gpu_allocator::AllocatorDebugSettings::default(),
            // Enabled on the device, so the allocator must know.
            buffer_device_address: true,
            allocation_sizes: gpu_allocator::AllocationSizes::default(),
        })
        .map_err(|e| RenderError::Allocator(e.to_string()))?;

        let (color, color_alloc) = create_image(
            &raw,
            &mut allocator,
            width,
            height,
            COLOR_FORMAT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            "loom.color_target",
        )?;
        let (depth, depth_alloc) = create_image(
            &raw,
            &mut allocator,
            width,
            height,
            DEPTH_FORMAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            "loom.depth_target",
        )?;

        let color_view = create_view(&raw, color, COLOR_FORMAT, vk::ImageAspectFlags::COLOR)?;
        let depth_view = create_view(&raw, depth, DEPTH_FORMAT, vk::ImageAspectFlags::DEPTH)?;

        // Host-visible landing zone for the finished image.
        let size = u64::from(width) * u64::from(height) * 4;
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `buffer_info` is fully initialised and outlives the call.
        let readback = unsafe { raw.create_buffer(&buffer_info, None) }?;
        // SAFETY: `readback` was just created on this device.
        let requirements = unsafe { raw.get_buffer_memory_requirements(readback) };
        let readback_alloc = allocator
            .allocate(&AllocationCreateDesc {
                name: "loom.readback",
                requirements,
                location: MemoryLocation::GpuToCpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| RenderError::Allocator(e.to_string()))?;
        // SAFETY: the allocation matches the buffer's requirements.
        unsafe { raw.bind_buffer_memory(readback, readback_alloc.memory(), readback_alloc.offset()) }?;

        // Geometry, uploaded once. Host-visible rather than device-local with a
        // staging copy: 36 vertices is far below the point where that matters,
        // and the transfer queue + staging ring arrive with real assets at M5.
        // `ponytail:` host-visible upload, upgrade when meshes get big.
        let cube = cube_vertices();
        let (vertices, vertices_alloc, vertex_address) = create_address_buffer(
            &raw,
            &mut allocator,
            std::mem::size_of_val(cube.as_slice()) as u64,
            "loom.cube_vertices",
        )?;
        write_slice(&vertices_alloc, &cube)?;

        // Per-object array. Sized once; the draw is a single instanced call.
        const MAX_OBJECTS: usize = 4096;
        let (objects, objects_alloc, object_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (MAX_OBJECTS * size_of::<ObjectData>()) as u64,
            "loom.object_data",
        )?;

        let cache_path = pipeline_cache_path(instance, device);
        let (pipeline_layout, pipeline, pipeline_cache) =
            create_pipeline(&raw, cache_path.as_deref(), COLOR_FORMAT)?;

        names.set(color, "loom.color_target");
        names.set(depth, "loom.depth_target");
        names.set(color_view, "loom.color_target.view");
        names.set(depth_view, "loom.depth_target.view");
        names.set(readback, "loom.readback_buffer");
        names.set(vertices, "loom.cube_vertices");
        names.set(objects, "loom.object_data");
        names.set(pipeline, "loom.scene_pipeline");
        names.set(pipeline_layout, "loom.scene_pipeline_layout");
        names.set(pipeline_cache, "loom.pipeline_cache");

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(device.queue_family())
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the family index came from this device.
        let command_pool = unsafe { raw.create_command_pool(&pool_info, None) }?;

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: pool is live.
        let command_buffer = unsafe { raw.allocate_command_buffers(&alloc_info) }?[0];

        // SAFETY: default create-info is valid.
        let fence = unsafe { raw.create_fence(&vk::FenceCreateInfo::default(), None) }?;

        names.set(command_pool, "loom.command_pool");
        names.set(command_buffer, "loom.command_buffer");
        names.set(fence, "loom.frame_fence");

        Ok(Self {
            device: raw,
            queue: device.queue(),
            allocator: Some(allocator),
            width,
            height,
            color,
            color_view,
            color_alloc: Some(color_alloc),
            depth,
            depth_view,
            depth_alloc: Some(depth_alloc),
            readback,
            readback_alloc: Some(readback_alloc),
            vertices,
            vertices_alloc: Some(vertices_alloc),
            vertex_address,
            objects,
            objects_alloc: Some(objects_alloc),
            object_address,
            max_objects: MAX_OBJECTS,
            pipeline_layout,
            pipeline,
            pipeline_cache,
            command_pool,
            command_buffer,
            fence,
            cache_path,
            last_transitions: Vec::new(),
        })
    }

    /// Render `objects` and return the image as RGBA8 rows, top to bottom.
    ///
    /// # Errors
    /// [`RenderError`] on any Vulkan failure.
    #[allow(clippy::too_many_lines)]
    pub fn render(&mut self, objects: &[Object], camera: &Camera) -> Result<Vec<u8>, RenderError> {
        if objects.len() > self.max_objects {
            return Err(RenderError::Allocator(format!(
                "{} objects exceeds the {} the object buffer was sized for",
                objects.len(),
                self.max_objects
            )));
        }
        let cmd = self.command_buffer;

        #[allow(clippy::cast_precision_loss)]
        let aspect = self.width as f32 / self.height as f32;
        let view_proj = view_projection(camera, aspect);

        // Per-object data goes to the GPU once, not once per draw.
        let object_data = pack_objects(objects, view_proj);
        write_slice(
            self.objects_alloc
                .as_ref()
                .ok_or_else(|| RenderError::Allocator("object buffer missing".into()))?,
            &object_data,
        )?;

        // Every barrier below is chosen by the graph, not written here.
        // never-do #4: no barrier lives outside it. What each pass *touches*
        // is declared; what that requires is derived.
        let mut graph = RenderGraph::new();
        let color = graph.import("loom.color_target", self.color);
        let depth = graph.import("loom.depth_target", self.depth);

        let (width, height) = (self.width, self.height);
        let (color_view, depth_view) = (self.color_view, self.depth_view);
        let (pipeline, layout) = (self.pipeline, self.pipeline_layout);
        let push = Push {
            vertices: self.vertex_address,
            objects: self.object_address,
        };
        let instances = u32::try_from(objects.len()).unwrap_or(0);
        let (readback, image) = (self.readback, self.color);

        graph.pass(
            "forward",
            &[(color, Access::ColorWrite), (depth, Access::DepthWrite)],
            move |d, cmd| {
                // SAFETY: the graph has already transitioned both attachments
                // into the layouts this recording requires.
                unsafe {
                    begin_rendering(d, cmd, color_view, depth_view, width, height);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    set_viewport(d, cmd, width, height);
                    let bytes = std::slice::from_raw_parts(
                        std::ptr::from_ref(&push).cast::<u8>(),
                        size_of::<Push>(),
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

        graph.pass("readback", &[(color, Access::TransferSrc)], move |d, cmd| {
            let region = vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(0)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
            // SAFETY: the graph put the image in TRANSFER_SRC_OPTIMAL.
            unsafe {
                d.cmd_copy_image_to_buffer(
                    cmd,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    readback,
                    &[region],
                );
            }
        });

        let d = &self.device;
        // SAFETY: the buffer is not in flight — the previous submit was waited on.
        unsafe {
            d.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            let begin = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            d.begin_command_buffer(cmd, &begin)?;
        }

        self.last_transitions = graph.execute(d, cmd);

        // SAFETY: recording is complete; nothing else uses this buffer.
        unsafe {
            d.end_command_buffer(cmd)?;
            let buffers = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&buffers);
            d.reset_fences(&[self.fence])?;
            d.queue_submit(self.queue, &[submit], self.fence)?;
            // Headless and single-frame, so a full stall is correct rather than
            // lazy: nothing can proceed until the pixels exist.
            d.wait_for_fences(&[self.fence], true, u64::MAX)?;
        }

        let allocation = self
            .readback_alloc
            .as_ref()
            .ok_or_else(|| RenderError::Allocator("readback allocation missing".into()))?;
        let mapped = allocation
            .mapped_ptr()
            .ok_or_else(|| RenderError::Allocator("readback memory is not host-visible".into()))?;

        let len = (self.width as usize) * (self.height as usize) * 4;
        // SAFETY: the copy above wrote exactly `len` bytes, and the fence has
        // been waited on, so the write is visible to the host.
        let pixels = unsafe { std::slice::from_raw_parts(mapped.as_ptr().cast::<u8>(), len) };
        Ok(pixels.to_vec())
    }

    /// The layout transitions the graph emitted on the last [`Self::render`].
    #[must_use]
    pub fn last_transitions(&self) -> &[Transition] {
        &self.last_transitions
    }

    /// Render and write a PNG.
    ///
    /// # Errors
    /// [`RenderError`] on render or IO failure.
    pub fn render_to_png(
        &mut self,
        objects: &[Object],
        camera: &Camera,
        path: &std::path::Path,
    ) -> Result<(), RenderError> {
        let pixels = self.render(objects, camera)?;
        let file = std::fs::File::create(path).map_err(RenderError::Io)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| {
            RenderError::Io(std::io::Error::other(e.to_string()))
        })?;
        writer.write_image_data(&pixels).map_err(|e| {
            RenderError::Io(std::io::Error::other(e.to_string()))
        })?;
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: idle first, then destroy in reverse creation order. Freeing
        // allocations before destroying their owning objects would be a
        // use-after-free the compiler cannot see.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.save_pipeline_cache();
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_cache(self.pipeline_cache, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_image_view(self.color_view, None);
            self.device.destroy_image_view(self.depth_view, None);

            if let Some(allocator) = self.allocator.as_mut() {
                for allocation in [
                    self.color_alloc.take(),
                    self.depth_alloc.take(),
                    self.readback_alloc.take(),
                    self.vertices_alloc.take(),
                    self.objects_alloc.take(),
                ]
                .into_iter()
                .flatten()
                {
                    let _ = allocator.free(allocation);
                }
            }
            self.device.destroy_image(self.color, None);
            self.device.destroy_image(self.depth, None);
            self.device.destroy_buffer(self.readback, None);
            self.device.destroy_buffer(self.vertices, None);
            self.device.destroy_buffer(self.objects, None);
            // The allocator must go before the device it borrows.
            self.allocator = None;
        }
    }
}

impl Renderer {
    /// Write the pipeline cache back to disk (Vulkan doc §9 step 3).
    ///
    /// Best-effort: a cache that cannot be written costs a recompile next run,
    /// which is not worth failing over.
    fn save_pipeline_cache(&self) {
        let Some(path) = &self.cache_path else { return };
        // SAFETY: the cache is live and idle.
        // SAFETY: the cache is live and the device is idle.
        let Ok(data) = (unsafe { self.device.get_pipeline_cache_data(self.pipeline_cache) }) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, data);
    }
}

/// Where this device's pipeline cache lives.
///
/// Vulkan doc §9 step 4: **key the file by driver version + device ID**, plus a
/// hash of the shader set. A stale cache is silently ignored at best and a
/// correctness hazard at worst, so a driver update must not be able to load the
/// previous driver's blob.
pub(crate) fn pipeline_cache_path(instance: &Instance, device: &Device) -> Option<std::path::PathBuf> {
    // SAFETY: the physical device came from this instance.
    let props = unsafe {
        instance
            .handle()
            .get_physical_device_properties(device.physical())
    };

    // FNV-1a over the shader bytes: not cryptographic, just needs to change
    // when the shaders do.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in crate::SCENE_SPV.iter().chain(crate::TRIANGLE_SPV) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }

    let key = format!(
        "{:08x}-{:08x}-{:08x}-{:016x}",
        props.vendor_id, props.device_id, props.driver_version, hash
    );
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    Some(base.join("loom").join(format!("pipeline-{key}.bin")))
}

/// Begin dynamic rendering with a colour and depth attachment, both cleared.
///
/// # Safety
/// Both views must already be in the layouts named here — which is the graph's
/// job, not this function's.
pub(crate) unsafe fn begin_rendering(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    color_view: vk::ImageView,
    depth_view: vk::ImageView,
    width: u32,
    height: u32,
) {
    let color_attachment = vk::RenderingAttachmentInfo::default()
        .image_view(color_view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .clear_value(vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.05, 0.06, 0.08, 1.0],
            },
        });
    let depth_attachment = vk::RenderingAttachmentInfo::default()
        .image_view(depth_view)
        .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::DONT_CARE)
        .clear_value(vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        });
    let color_attachments = [color_attachment];
    let rendering = vk::RenderingInfo::default()
        .render_area(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D { width, height },
        })
        .layer_count(1)
        .color_attachments(&color_attachments)
        .depth_attachment(&depth_attachment);
    unsafe { d.cmd_begin_rendering(cmd, &rendering) };
}

/// # Safety
/// `cmd` must be recording, with a pipeline bound that uses dynamic state.
pub(crate) unsafe fn set_viewport(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    width: u32,
    height: u32,
) {
    #[allow(clippy::cast_precision_loss)]
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: width as f32,
        height: height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D { width, height },
    };
    unsafe {
        d.cmd_set_viewport(cmd, 0, &[viewport]);
        d.cmd_set_scissor(cmd, 0, &[scissor]);
    }
}

/// Build the per-object array the shader indexes with `SV_InstanceID`.
pub(crate) fn pack_objects(objects: &[Object], view_proj: Mat4) -> Vec<ObjectData> {
    objects
        .iter()
        .map(|object| {
            // Inverse-transpose, because non-uniform scale skews normals.
            let normal_matrix = glam::Mat3::from_mat4(object.model).inverse().transpose();
            // Transposing gives columns that are the original's rows.
            let rows = normal_matrix.transpose();
            ObjectData {
                mvp: (view_proj * object.model).to_cols_array(),
                normal: [
                    rows.x_axis.extend(0.0).to_array(),
                    rows.y_axis.extend(0.0).to_array(),
                    rows.z_axis.extend(0.0).to_array(),
                ],
                color: [object.color[0], object.color[1], object.color[2], 1.0],
            }
        })
        .collect()
}

/// The camera's view-projection, with Vulkan's Y flip applied.
pub(crate) fn view_projection(camera: &Camera, aspect: f32) -> Mat4 {
    let view = Mat4::look_at_rh(camera.eye, camera.target, Vec3::Y);
    let mut proj = Mat4::perspective_rh(camera.fov_y_degrees.to_radians(), aspect, 0.1, 1000.0);
    // Vulkan NDC has +Y down; flipping here keeps world space Y-up, which is
    // the convention pinned in the format spec.
    proj.y_axis.y *= -1.0;
    proj * view
}

/// A host-visible buffer that the shader reaches by device address.
pub(crate) fn create_address_buffer(
    device: &ash::Device,
    allocator: &mut Allocator,
    size: u64,
    name: &str,
) -> Result<(vk::Buffer, Allocation, vk::DeviceAddress), RenderError> {
    let info = vk::BufferCreateInfo::default()
        .size(size)
        // SHADER_DEVICE_ADDRESS is what makes `vkGetBufferDeviceAddress` legal
        // on this buffer; without it the call is a validation error.
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: `info` is fully initialised and outlives the call.
    let buffer = unsafe { device.create_buffer(&info, None) }?;
    // SAFETY: `buffer` was just created on this device.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let allocation = allocator
        .allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| RenderError::Allocator(e.to_string()))?;
    // SAFETY: the allocation matches the buffer's requirements.
    unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }?;

    let address_info = vk::BufferDeviceAddressInfo::default().buffer(buffer);
    // SAFETY: the buffer is bound and was created with SHADER_DEVICE_ADDRESS.
    let address = unsafe { device.get_buffer_device_address(&address_info) };
    Ok((buffer, allocation, address))
}

/// Copy a `#[repr(C)]` slice into a mapped allocation.
pub(crate) fn write_slice<T: Copy>(allocation: &Allocation, data: &[T]) -> Result<(), RenderError> {
    let mapped = allocation
        .mapped_ptr()
        .ok_or_else(|| RenderError::Allocator("buffer is not host-visible".into()))?;
    // SAFETY: the allocation was sized for at least `data`, `T` is `Copy` and
    // `#[repr(C)]`, and the destination is uninitialised bytes we fully write.
    unsafe {
        std::ptr::copy_nonoverlapping(
            data.as_ptr().cast::<u8>(),
            mapped.as_ptr().cast::<u8>(),
            std::mem::size_of_val(data),
        );
    }
    Ok(())
}

pub(crate) fn create_image(
    device: &ash::Device,
    allocator: &mut Allocator,
    width: u32,
    height: u32,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    name: &str,
) -> Result<(vk::Image, Allocation), RenderError> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);

    // SAFETY: `info` is fully initialised and outlives the call.
    let image = unsafe { device.create_image(&info, None) }?;
    // SAFETY: `image` was just created on this device.
    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let allocation = allocator
        .allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| RenderError::Allocator(e.to_string()))?;
    // SAFETY: the allocation matches the image's requirements.
    unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset()) }?;
    Ok((image, allocation))
}

pub(crate) fn create_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
) -> Result<vk::ImageView, RenderError> {
    let info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    // SAFETY: `info` is fully initialised and `image` is live.
    Ok(unsafe { device.create_image_view(&info, None) }?)
}

/// Build the graphics pipeline. Dynamic rendering, so no render pass object.
pub(crate) fn create_pipeline(
    device: &ash::Device,
    cache_path: Option<&std::path::Path>,
    color_format: vk::Format,
) -> Result<(vk::PipelineLayout, vk::Pipeline, vk::PipelineCache), RenderError> {
    let module = create_shader_module(device, crate::SCENE_SPV)?;

    let push_range = vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
        .offset(0)
        .size(u32::try_from(size_of::<Push>()).unwrap_or(128));
    let ranges = [push_range];
    let layout_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&ranges);
    // SAFETY: `ranges` outlives the call.
    let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }?;

    // Vulkan doc §9: create the cache up front and pass it to every pipeline
    // creation. Retrofitting means auditing every creation site.
    let previous = cache_path.and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    let cache_info = vk::PipelineCacheCreateInfo::default().initial_data(&previous);
    // SAFETY: `previous` outlives the call. A corrupt or foreign blob is
    // rejected by the driver and simply ignored, which is why the path is
    // keyed by driver and device (see `pipeline_cache_path`).
    let cache = unsafe { device.create_pipeline_cache(&cache_info, None) }?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(c"vertexMain"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(c"fragmentMain"),
    ];

    // No vertex input: geometry comes from SV_VertexID in the shader.
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::BACK)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false);
    let attachments = [blend_attachment];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [color_format];
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(DEPTH_FORMAT);

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
    let pipeline = unsafe { device.create_graphics_pipelines(cache, &[info], None) }
        .map_err(|(_, r)| RenderError::Vulkan(r))?[0];

    // SAFETY: the module is baked into the pipeline and no longer needed.
    unsafe { device.destroy_shader_module(module, None) };

    Ok((layout, pipeline, cache))
}

fn create_shader_module(
    device: &ash::Device,
    spv: &[u8],
) -> Result<vk::ShaderModule, RenderError> {
    // SPIR-V is a stream of 32-bit words; `build.rs` guarantees alignment of
    // the embedded bytes only to 1, so copy into an aligned Vec.
    let words: Vec<u32> = spv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    // SAFETY: `words` outlives the call and is a valid SPIR-V module —
    // `build.rs` ran `spirv-val` on it.
    Ok(unsafe { device.create_shader_module(&info, None) }?)
}

