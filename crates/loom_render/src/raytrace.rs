//! Acceleration structures, for shadows traced from the fragment shader.
//!
//! **Ray query, not a ray tracing pipeline.** The rays here are secondary —
//! "is this point in shadow" — and a ray query answers that inline from the
//! shader already shading the pixel. The pipeline variant would add a shader
//! binding table, three new shader stages and a second pipeline to reach the
//! same answer, and measures slower for pure visibility. Raster for primary
//! visibility plus ray queries for secondary is what the extension is for.
//!
//! Two levels, as Vulkan requires:
//!
//! - **BLAS**, one per mesh, built once when the mesh set changes. It holds the
//!   triangles in the mesh's own space, so it survives the object moving.
//! - **TLAS**, one, rebuilt whenever objects move. It holds an instance per
//!   drawn object: a transform and a pointer to a BLAS. Rebuilding this is
//!   cheap precisely because the triangles live in the BLAS and are not
//!   touched.
//!
//! This is the one place in the renderer with a descriptor set. Never-do #2
//! forbids *per-draw* descriptor sets; an acceleration structure has no buffer
//! device address form, so a single set bound once per frame is the only way to
//! reach it, and it is what the bindless model does for its own global tables.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::renderer::{MeshRange, create_address_buffer, write_slice};
use crate::{Object, RenderError};

/// An acceleration structure plus the memory it lives in.
struct Structure {
    handle: vk::AccelerationStructureKHR,
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    address: vk::DeviceAddress,
}

/// Everything needed to trace rays against the current scene.
pub(crate) struct Raytracer {
    device: ash::Device,
    accel: ash::khr::acceleration_structure::Device,
    blas: Vec<Structure>,
    tlas: Option<Structure>,
    /// Instance descriptions, host-visible and rewritten whenever objects move.
    instances: vk::Buffer,
    instances_alloc: Option<Allocation>,
    instances_address: vk::DeviceAddress,
    instance_capacity: usize,
    /// Build scratch. Kept rather than reallocated per build.
    scratch: vk::Buffer,
    scratch_alloc: Option<Allocation>,
    scratch_address: vk::DeviceAddress,
    scratch_size: vk::DeviceSize,
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
}

impl Raytracer {
    /// Create the descriptor plumbing. No structures are built yet.
    ///
    /// # Errors
    /// [`RenderError`] if a Vulkan object or allocation fails.
    pub(crate) fn new(
        instance: &ash::Instance,
        device: &ash::Device,
        allocator: &mut Allocator,
    ) -> Result<Self, RenderError> {
        let accel = ash::khr::acceleration_structure::Device::new(instance, device);

        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let bindings = [binding];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: `layout_info` is fully initialised and outlives the call.
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }?;

        let size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .descriptor_count(1);
        let sizes = [size];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&sizes);
        // SAFETY: `pool_info` is fully initialised and outlives the call.
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }?;

        let layouts = [layout];
        let allocate = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: the pool has room for exactly this one set.
        let set = unsafe { device.allocate_descriptor_sets(&allocate) }?[0];

        // Sized on first use; these are placeholders so `Drop` is uniform.
        let (instances, instances_alloc, instances_address) = create_address_buffer(
            device,
            allocator,
            INSTANCE_SIZE as u64,
            "loom.rt_instances",
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
        )?;
        let (scratch, scratch_alloc, scratch_address) = create_address_buffer(
            device,
            allocator,
            SCRATCH_MIN,
            "loom.rt_scratch",
            vk::BufferUsageFlags::empty(),
        )?;

        Ok(Self {
            device: device.clone(),
            accel,
            blas: Vec::new(),
            tlas: None,
            instances,
            instances_alloc: Some(instances_alloc),
            instances_address,
            instance_capacity: 1,
            scratch,
            scratch_alloc: Some(scratch_alloc),
            scratch_address,
            scratch_size: SCRATCH_MIN,
            layout,
            pool,
            set,
        })
    }

    pub(crate) fn descriptor_layout(&self) -> vk::DescriptorSetLayout {
        self.layout
    }

    pub(crate) fn descriptor_set(&self) -> vk::DescriptorSet {
        self.set
    }

    /// Whether there is anything to trace against.
    pub(crate) fn ready(&self) -> bool {
        self.tlas.is_some()
    }

    /// Build one BLAS per mesh. Call when the mesh set changes, not per frame.
    ///
    /// # Errors
    /// [`RenderError`] if a build or allocation fails.
    pub(crate) fn build_meshes(
        &mut self,
        allocator: &mut Allocator,
        target: Submit,
        vertex_address: vk::DeviceAddress,
        index_address: vk::DeviceAddress,
        ranges: &[MeshRange],
        vertex_count: u32,
    ) -> Result<(), RenderError> {
        self.free_blas(allocator);
        if ranges.is_empty() || vertex_count == 0 {
            return Ok(());
        }

        for range in ranges {
            let triangles = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
                // Position is the first three floats of the vertex; the stride
                // steps over the normal that follows it.
                .vertex_format(vk::Format::R32G32B32_SFLOAT)
                .vertex_data(vk::DeviceOrHostAddressConstKHR {
                    device_address: vertex_address,
                })
                // Tightly packed float3 positions; see `rt_positions`.
                .vertex_stride(std::mem::size_of::<[f32; 3]>() as u64)
                .max_vertex(vertex_count - 1)
                .index_type(vk::IndexType::UINT32)
                .index_data(vk::DeviceOrHostAddressConstKHR {
                    device_address: index_address,
                });
            let geometry = vk::AccelerationStructureGeometryKHR::default()
                .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
                .geometry(vk::AccelerationStructureGeometryDataKHR { triangles })
                // OPAQUE lets traversal stop at the first hit without calling
                // back into a shader — the whole reason a shadow ray is cheap.
                .flags(vk::GeometryFlagsKHR::OPAQUE);
            let geometries = [geometry];

            let primitives = range.index_count() / 3;
            let build = vk::AccelerationStructureBuildGeometryInfoKHR::default()
                .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
                .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
                .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
                .geometries(&geometries);

            let structure = self.build_one(
                allocator,
                target,
                build,
                vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
                primitives,
                // Byte offset into the index buffer for this mesh's first index.
                vk::AccelerationStructureBuildRangeInfoKHR::default()
                    .primitive_count(primitives)
                    .primitive_offset(range.first_index() * 4),
            )?;
            self.blas.push(structure);
        }
        Ok(())
    }

    /// Rebuild the TLAS for the objects as they are now.
    ///
    /// # Errors
    /// [`RenderError`] if a build or allocation fails.
    pub(crate) fn build_instances(
        &mut self,
        allocator: &mut Allocator,
        target: Submit,
        objects: &[Object],
    ) -> Result<(), RenderError> {
        if self.blas.is_empty() || objects.is_empty() {
            return Ok(());
        }

        let instances: Vec<vk::AccelerationStructureInstanceKHR> = objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| {
                let blas = self.blas.get(object.mesh as usize)?;
                // Vulkan wants a 3x4 row-major transform; glam is column-major,
                // so this transposes as it copies. Getting it wrong puts every
                // shadow somewhere else in the scene — and nothing validates it.
                let m = object.model.to_cols_array();
                let transform = vk::TransformMatrixKHR {
                    matrix: [
                        m[0], m[4], m[8], m[12], //
                        m[1], m[5], m[9], m[13], //
                        m[2], m[6], m[10], m[14],
                    ],
                };
                Some(
                    vk::AccelerationStructureInstanceKHR {
                        transform,
                        // **The object's index, so a ray can find out what it
                        // hit.** A shadow ray never needed this — it asks only
                        // whether anything is in the way — but a reflection
                        // ray has to shade the surface it lands on, and
                        // `CommittedInstanceID()` is the only channel from
                        // traversal back to the shader that costs nothing.
                        //
                        // This index addresses `push.objects` directly:
                        // `pack_objects` and this function are both handed the
                        // same mesh-sorted slice, in the same order, by both
                        // the offscreen and the windowed path. Passing an
                        // unsorted list to one and not the other would put
                        // every reflection's colour on the wrong object, and
                        // nothing would validate it — so the two calls stay
                        // next to each other in `render`.
                        //
                        // 24 bits, against a 4096-object buffer that grows by
                        // doubling; `try_from` rather than `as` so an
                        // implausibly large scene truncates to object 0 rather
                        // than wrapping to an arbitrary one.
                        instance_custom_index_and_mask: vk::Packed24_8::new(
                            u32::try_from(index).unwrap_or(0) & 0x00ff_ffff,
                            0xff,
                        ),
                        instance_shader_binding_table_record_offset_and_flags:
                            vk::Packed24_8::new(0, 0),
                        acceleration_structure_reference: vk::AccelerationStructureReferenceKHR {
                            device_handle: blas.address,
                        },
                    },
                )
            })
            .collect();
        if instances.is_empty() {
            return Ok(());
        }

        if instances.len() > self.instance_capacity {
            self.grow_instances(allocator, instances.len())?;
        }
        if let Some(allocation) = self.instances_alloc.as_ref() {
            write_slice(allocation, &instances)?;
        }

        let data = vk::AccelerationStructureGeometryInstancesDataKHR::default().data(
            vk::DeviceOrHostAddressConstKHR {
                device_address: self.instances_address,
            },
        );
        let geometry = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::INSTANCES)
            .geometry(vk::AccelerationStructureGeometryDataKHR { instances: data })
            .flags(vk::GeometryFlagsKHR::OPAQUE);
        let geometries = [geometry];

        let count = u32::try_from(instances.len()).unwrap_or(u32::MAX);
        let build = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .geometries(&geometries);

        let structure = self.build_one(
            allocator,
            target,
            build,
            vk::AccelerationStructureTypeKHR::TOP_LEVEL,
            count,
            vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(count),
        )?;

        // Retire the previous TLAS only once the new one exists, so a failed
        // build leaves the last good structure in place rather than none.
        if let Some(old) = self.tlas.replace(structure) {
            self.free_structure(allocator, old);
        }
        self.write_descriptor();
        Ok(())
    }

    /// Point the descriptor at the current TLAS.
    fn write_descriptor(&self) {
        let Some(tlas) = self.tlas.as_ref() else {
            return;
        };
        let structures = [tlas.handle];
        let mut accel_write =
            vk::WriteDescriptorSetAccelerationStructureKHR::default().acceleration_structures(&structures);
        let mut write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .push_next(&mut accel_write);
        // `push_next` cannot know the count, and the builder derives it from a
        // slice this struct does not have. Set it by hand or the write is a
        // no-op the validation layers will not flag.
        write.descriptor_count = 1;
        // SAFETY: the set and structure both outlive this call.
        unsafe { self.device.update_descriptor_sets(&[write], &[]) };
    }

    /// Size, create, and build one acceleration structure.
    fn build_one(
        &mut self,
        allocator: &mut Allocator,
        target: Submit,
        mut build: vk::AccelerationStructureBuildGeometryInfoKHR<'_>,
        kind: vk::AccelerationStructureTypeKHR,
        primitives: u32,
        range: vk::AccelerationStructureBuildRangeInfoKHR,
    ) -> Result<Structure, RenderError> {
        let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
        // SAFETY: `build` has one geometry, matching the one count below.
        unsafe {
            self.accel.get_acceleration_structure_build_sizes(
                vk::AccelerationStructureBuildTypeKHR::DEVICE,
                &build,
                &[primitives],
                &mut sizes,
            );
        }

        let (buffer, allocation, _) = create_address_buffer(
            &self.device,
            allocator,
            sizes.acceleration_structure_size.max(4),
            "loom.rt_structure",
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR,
        )?;

        let create_info = vk::AccelerationStructureCreateInfoKHR::default()
            .buffer(buffer)
            .size(sizes.acceleration_structure_size)
            .ty(kind);
        // SAFETY: `buffer` was just created with ACCELERATION_STRUCTURE_STORAGE.
        let handle = unsafe { self.accel.create_acceleration_structure(&create_info, None) }?;

        self.reserve_scratch(allocator, sizes.build_scratch_size)?;

        build = build
            .dst_acceleration_structure(handle)
            .scratch_data(vk::DeviceOrHostAddressKHR {
                device_address: self.scratch_address,
            });

        self.submit_build(target, &build, &range)?;

        let address_info =
            vk::AccelerationStructureDeviceAddressInfoKHR::default().acceleration_structure(handle);
        // SAFETY: `handle` was just built on this device.
        let address = unsafe {
            self.accel
                .get_acceleration_structure_device_address(&address_info)
        };

        Ok(Structure {
            handle,
            buffer,
            allocation: Some(allocation),
            address,
        })
    }

    /// Record and wait on one build. Builds are rare; a fence beats a pipeline.
    fn submit_build(
        &self,
        target: Submit,
        build: &vk::AccelerationStructureBuildGeometryInfoKHR<'_>,
        range: &vk::AccelerationStructureBuildRangeInfoKHR,
    ) -> Result<(), RenderError> {
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(target.pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool belongs to this device.
        let cmd = unsafe { self.device.allocate_command_buffers(&allocate) }?[0];

        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: `cmd` was just allocated and is not recording.
        unsafe { self.device.begin_command_buffer(cmd, &begin) }?;
        // SAFETY: every address in `build` refers to a live buffer.
        unsafe {
            self.accel
                .cmd_build_acceleration_structures(cmd, &[*build], &[&[*range]]);
        }
        // SAFETY: `cmd` is recording.
        unsafe { self.device.end_command_buffer(cmd) }?;

        let buffers = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&buffers);
        // SAFETY: the fence and command buffer outlive the wait below.
        let result = unsafe {
            let fence = self
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)?;
            let submitted = self.device.queue_submit(target.queue, &[submit], fence);
            let waited = submitted.and_then(|()| {
                self.device
                    .wait_for_fences(&[fence], true, u64::MAX)
            });
            self.device.destroy_fence(fence, None);
            self.device.free_command_buffers(target.pool, &buffers);
            waited
        };
        result.map_err(RenderError::Vulkan)
    }

    /// Make sure the shared scratch buffer is at least `size` bytes.
    fn reserve_scratch(
        &mut self,
        allocator: &mut Allocator,
        size: vk::DeviceSize,
    ) -> Result<(), RenderError> {
        if size <= self.scratch_size {
            return Ok(());
        }
        let wanted = size.max(self.scratch_size * 2);
        // Create before destroying, so a failed allocation leaves the old
        // buffer usable rather than leaving the renderer with neither.
        let (buffer, allocation, address) = create_address_buffer(
            &self.device,
            allocator,
            wanted,
            "loom.rt_scratch",
            vk::BufferUsageFlags::empty(),
        )?;
        if let Some(old) = self.scratch_alloc.take() {
            let _ = allocator.free(old);
        }
        // SAFETY: the old scratch is no longer referenced by any pending build;
        // every build waits on its own fence before returning.
        unsafe { self.device.destroy_buffer(self.scratch, None) };
        self.scratch = buffer;
        self.scratch_alloc = Some(allocation);
        self.scratch_address = address;
        self.scratch_size = wanted;
        Ok(())
    }

    fn grow_instances(
        &mut self,
        allocator: &mut Allocator,
        count: usize,
    ) -> Result<(), RenderError> {
        let (buffer, allocation, address) = create_address_buffer(
            &self.device,
            allocator,
            (count * INSTANCE_SIZE) as u64,
            "loom.rt_instances",
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
        )?;
        if let Some(old) = self.instances_alloc.take() {
            let _ = allocator.free(old);
        }
        // SAFETY: no build is in flight; each waits on its own fence.
        unsafe { self.device.destroy_buffer(self.instances, None) };
        self.instances = buffer;
        self.instances_alloc = Some(allocation);
        self.instances_address = address;
        self.instance_capacity = count;
        Ok(())
    }

    fn free_structure(&self, allocator: &mut Allocator, mut structure: Structure) {
        // SAFETY: nothing references this structure; builds are fenced.
        unsafe {
            self.accel
                .destroy_acceleration_structure(structure.handle, None);
            self.device.destroy_buffer(structure.buffer, None);
        }
        if let Some(allocation) = structure.allocation.take() {
            let _ = allocator.free(allocation);
        }
    }

    fn free_blas(&mut self, allocator: &mut Allocator) {
        for structure in std::mem::take(&mut self.blas) {
            self.free_structure(allocator, structure);
        }
    }

    /// Release everything. Called before the allocator itself goes away.
    pub(crate) fn destroy(&mut self, allocator: &mut Allocator) {
        self.free_blas(allocator);
        if let Some(tlas) = self.tlas.take() {
            self.free_structure(allocator, tlas);
        }
        if let Some(allocation) = self.instances_alloc.take() {
            let _ = allocator.free(allocation);
        }
        if let Some(allocation) = self.scratch_alloc.take() {
            let _ = allocator.free(allocation);
        }
        // SAFETY: every structure above is already destroyed, and the device is
        // idle — the caller waits before tearing down.
        unsafe {
            self.device.destroy_buffer(self.instances, None);
            self.device.destroy_buffer(self.scratch, None);
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}

/// Where a one-off build is recorded and submitted.
#[derive(Clone, Copy)]
pub(crate) struct Submit {
    pub(crate) pool: vk::CommandPool,
    pub(crate) queue: vk::Queue,
}

/// One `VkAccelerationStructureInstanceKHR`.
const INSTANCE_SIZE: usize = std::mem::size_of::<vk::AccelerationStructureInstanceKHR>();

/// Enough scratch that the first small build never reallocates.
const SCRATCH_MIN: vk::DeviceSize = 1 << 20;
