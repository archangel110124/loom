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

/// What a TLAS instance actually is: where it sits, which object it names, and
/// which BLAS it points at. Every other field of
/// [`vk::AccelerationStructureInstanceKHR`] is a constant here.
type InstanceKey = ([f32; 12], u32, vk::DeviceAddress);

/// What the TLAS would be built from, for these objects, right now.
///
/// **A free function so the skip has a test.** `build_instances` needs a
/// device, a queue and a fence, so nothing in the five green checks can reach
/// it: `render_to_png` calls `render` once, every GOLDEN row is one frame, and
/// on a first frame `tlas` is `None` and the skip branch is never taken. A
/// fault injected into the comparison broke six of eight frames on four scenes
/// and passed all five checks unchanged. This is the half of it that is
/// arithmetic, and `tests` below is what fails when the arithmetic moves.
fn instance_keys(objects: &[Object], cutout: &[bool], blas: &[Structure]) -> Vec<InstanceKey> {
    objects
        .iter()
        .enumerate()
        .filter_map(|(index, object)| {
            // **An alpha-tested surface is kept out of the structure
            // entirely.** A ray query never runs a fragment shader, so it would
            // occlude as the full quad the leaf was cut from — a canopy of a
            // thousand cards laying a thousand hard rectangles on the ground,
            // and shadowing itself with them too.
            //
            // No shadow is not free either; it is simply the lesser wrong, and
            // the honest one. Doing this properly means committing non-opaque
            // hits from inside the traversal loop, which needs the hit
            // triangle's UVs — and ADR 0021 records that a ray hit in this
            // engine has no UVs to read. That is an ADR, not a patch.
            //
            // The index still comes from `enumerate` over the *whole* slice, so
            // skipping one does not shift any other instance's custom index
            // away from its object.
            if cutout.get(index).copied().unwrap_or(false) {
                return None;
            }
            let blas = blas.get(object.mesh as usize)?;
            // Vulkan wants a 3x4 row-major transform; glam is column-major, so
            // this transposes as it copies. Getting it wrong puts every shadow
            // somewhere else in the scene — and nothing validates it.
            let m = object.model.to_cols_array();
            let transform = [
                m[0], m[4], m[8], m[12], //
                m[1], m[5], m[9], m[13], //
                m[2], m[6], m[10], m[14],
            ];
            // 24 bits, against a 4096-object buffer that grows by doubling;
            // `try_from` rather than `as` so an implausibly large scene
            // truncates to object 0 rather than wrapping to an arbitrary one.
            let custom_index = u32::try_from(index).unwrap_or(0) & 0x00ff_ffff;
            Some((transform, custom_index, blas.address))
        })
        .collect()
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
    /// The keys the live TLAS was built from.
    ///
    /// **Rebuilding an unchanged structure costs ~0.5 ms of blocking submit
    /// per frame and moves no pixel.** `build_instances` used to be called
    /// unconditionally from `renderer.rs` every frame, and `submit_build`
    /// waits on its own fence — so a static scene paid a serial stall larger
    /// than its entire secondary-ray bill for a structure identical to the one
    /// it replaced. Measured at 1920x1080 as the difference in per-frame wall
    /// clock with and without the skip; see the table in the commit.
    ///
    /// It self-invalidates: the transform is in the key, so anything that
    /// moves rebuilds, and the BLAS address is in it too. `build_meshes` sets
    /// it to `None`, because a freed BLAS can be reallocated at the same
    /// address with different geometry.
    ///
    /// **`None` rather than an empty list, and the difference is a scene the
    /// engine cannot load today.** Clearing it made "nothing has been built"
    /// and "the last build had no instances" the same value, so a `build_meshes`
    /// onto a live `Raytracer` whose new scene produced no instances — no
    /// meshes, or all of them alpha-cutout — would compare equal, skip, and
    /// leave the *previous* scene's TLAS bound for every ray in the frame.
    /// Unreachable while both call sites sit in `new()` and `tlas` is therefore
    /// `None`; one word rather than a comment saying so, because the day that
    /// stops being true nothing would report it.
    built: Option<Vec<InstanceKey>>,
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
            built: None,
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
        // Every instance key holds a BLAS address, and these are the addresses
        // being freed. A new BLAS can land on a freed one, so the keys cannot
        // be trusted across this.
        self.built = None;
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
        cutout: &[bool],
    ) -> Result<(), RenderError> {
        // **No early return for an empty scene.** See the note further down:
        // the TLAS has to exist whenever the device can trace, because a
        // shader's *static* use of `sceneTLAS` is a property of the shader and
        // not of what the scene happens to hold.
        let keys = instance_keys(objects, cutout, &self.blas);
        // **Nothing moved, so nothing is rebuilt.** See [`Raytracer::built`].
        // `tlas.is_some()` is what keeps the first frame honest — and an empty
        // scene still gets its zero-instance TLAS built once, for the reason
        // spelled out below. It is not redundant beside the `Option`: `built`
        // is assigned before the build is submitted, so a build that then fails
        // leaves keys recorded for a structure that does not exist.
        if self.tlas.is_some() && self.built.as_ref() == Some(&keys) {
            return Ok(());
        }
        // **Built from the keys, not beside them.** A key holds everything a
        // TLAS instance carries that varies — transform, object index, BLAS
        // address — so deriving one from the other is what stops the skip from
        // comparing something the build does not use. It also puts this work
        // after the early return, where a static scene never does it at all.
        let instances: Vec<vk::AccelerationStructureInstanceKHR> = keys
            .iter()
            .map(
                |&(matrix, custom_index, address)| vk::AccelerationStructureInstanceKHR {
                    transform: vk::TransformMatrixKHR { matrix },
                    // **The object's index, so a ray can find out what it
                    // hit.** A shadow ray never needed this — it asks only
                    // whether anything is in the way — but a reflection ray has
                    // to shade the surface it lands on, and
                    // `CommittedInstanceID()` is the only channel from
                    // traversal back to the shader that costs nothing.
                    //
                    // This index addresses `push.objects` directly:
                    // `pack_objects` and `instance_keys` are both handed the
                    // same mesh-sorted slice, in the same order, by both the
                    // offscreen and the windowed path. Passing an unsorted list
                    // to one and not the other would put every reflection's
                    // colour on the wrong object, and nothing would validate it
                    // — so the two calls stay next to each other in `render`.
                    instance_custom_index_and_mask: vk::Packed24_8::new(custom_index, 0xff),
                    instance_shader_binding_table_record_offset_and_flags: vk::Packed24_8::new(
                        0, 0,
                    ),
                    acceleration_structure_reference: vk::AccelerationStructureReferenceKHR {
                        device_handle: address,
                    },
                },
            )
            .collect();
        self.built = Some(keys);
        // **An empty scene still gets a TLAS, and that is a validation
        // requirement rather than tidiness.** This used to return early, so
        // `ready()` was false and `renderer.rs` skipped binding set 0 — which
        // was safe only while every pipeline that statically uses `sceneTLAS`
        // also had nothing to draw. That stopped being true when the water
        // fragment shader started tracing a reflection: `assets/test/ocean`
        // draws water and no meshes, and the draw is
        // `VUID-vkCmdDraw-None-08600`, "uses set 0 but that set is not bound".
        //
        // A scene of nothing but alpha-cutout meshes had the same shape
        // already — `cutout` objects are deliberately kept out of the TLAS, so
        // the opaque pipeline could draw with set 0 unbound — and no scene in
        // the repository happened to be one. Building a zero-instance TLAS
        // fixes both at the root: every ray misses, which is exactly the
        // behaviour `ready() == false` was standing in for, and there is no
        // longer a shader-visible state the descriptor is absent in.
        // The instance buffer is allocated even for a zero-instance build: the
        // geometry's `data` address has to be a real one, and `max(1)` is the
        // smallest that keeps `grow_instances` from being asked for nothing.
        if instances.len() > self.instance_capacity || self.instances_alloc.is_none() {
            self.grow_instances(allocator, instances.len().max(1))?;
        }
        if let (Some(allocation), false) = (self.instances_alloc.as_ref(), instances.is_empty()) {
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

#[cfg(test)]
mod tests {
    use super::{Structure, instance_keys};
    use crate::Object;
    use ash::vk;
    use glam::Mat4;

    /// A BLAS at a known address, and nothing else. `instance_keys` reads only
    /// `address`, which is the whole reason it can be tested without a device.
    fn blas(address: vk::DeviceAddress) -> Structure {
        Structure {
            handle: vk::AccelerationStructureKHR::null(),
            buffer: vk::Buffer::null(),
            allocation: None,
            address,
        }
    }

    fn object(mesh: u32, x: f32) -> Object {
        Object {
            model: Mat4::from_translation(glam::vec3(x, 0.0, 0.0)),
            color: [1.0; 3],
            mesh,
            material: u32::MAX,
            sway: 0.0,
            deform: [0.0; 4],
            deform_frame: [0.0; 4],
        }
    }

    /// The property the per-frame skip rests on: a scene that moved is not the
    /// scene it was. One millimetre is deliberate — the comparison is exact,
    /// and a tolerance here would be either a rebuild the frame did not need or
    /// a stale structure, depending on which side of it a scene fell.
    #[test]
    fn moving_an_object_changes_the_keys() {
        let structures = [blas(0x1000), blas(0x2000)];
        let still = [object(0, 0.0), object(1, 5.0)];
        let moved = [object(0, 0.001), object(1, 5.0)];
        let cutout = [false, false];
        assert_eq!(
            instance_keys(&still, &cutout, &structures),
            instance_keys(&still, &cutout, &structures),
            "the same objects must produce the same keys, or nothing is ever skipped"
        );
        assert_ne!(
            instance_keys(&still, &cutout, &structures),
            instance_keys(&moved, &cutout, &structures),
            "a moved object must rebuild the TLAS"
        );
    }

    /// Reordering is a change even when the set is identical, because the
    /// custom index addresses `push.objects` by position: two objects swapping
    /// places swaps what every reflection ray off them shades.
    #[test]
    fn reordering_changes_the_keys() {
        let structures = [blas(0x1000), blas(0x2000)];
        let cutout = [false, false];
        let forward = [object(0, 0.0), object(1, 5.0)];
        let backward = [object(1, 5.0), object(0, 0.0)];
        assert_ne!(
            instance_keys(&forward, &cutout, &structures),
            instance_keys(&backward, &cutout, &structures)
        );
    }

    /// A rebuilt BLAS at a new address is a new structure even for an object
    /// that never moved — which is why the address is in the key at all.
    #[test]
    fn a_rebuilt_blas_changes_the_keys() {
        let objects = [object(0, 0.0)];
        let cutout = [false];
        assert_ne!(
            instance_keys(&objects, &cutout, &[blas(0x1000)]),
            instance_keys(&objects, &cutout, &[blas(0x3000)])
        );
    }

    /// A cutout object leaves the structure but does not shift anyone else's
    /// custom index — `enumerate` runs over the whole slice for exactly this.
    #[test]
    fn cutout_is_skipped_without_shifting_the_rest() {
        let structures = [blas(0x1000), blas(0x2000), blas(0x3000)];
        let objects = [object(0, 0.0), object(1, 5.0), object(2, 9.0)];
        let keys = instance_keys(&objects, &[false, true, false], &structures);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].1, 0);
        assert_eq!(keys[1].1, 2, "the third object is still object 2");
    }
}
