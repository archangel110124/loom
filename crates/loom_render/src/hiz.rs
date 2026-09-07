//! Hi-Z occlusion culling: this frame's depth, reduced for the next frame's cull.
//!
//! **The cheap variant, chosen deliberately and with a known blind spot.** A
//! deterministic occlusion cull would need a depth prepass — draw everything to
//! depth, reduce, then cull the colour pass — and at this engine's scene sizes
//! (8–12 objects) that costs strictly more than it saves. This instead reuses
//! the depth the frame already produced, which is free, and pays for it by
//! being invisible to every single-frame render: a golden image has no previous
//! frame, so the grid is null and nothing is culled. See ADR 0084 for what that
//! means for verification, and `hiz_occludes_across_frames` for the multi-frame
//! test that does see it.
//!
//! Standard depth here (`CompareOp::LESS`, 0..1), so a cell keeps the
//! **farthest** depth it covers and an object is occluded only when its nearest
//! point is behind that. Being wrong in the other direction deletes something
//! visible, which is why every uncertain case is kept.

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::debug_names::DebugNames;
use crate::renderer::{RenderError, create_address_buffer_in, create_shader_module};

/// Cells per side. 64×64 floats is 16 KB — small enough that reading it on the
/// CPU each frame is free, coarse enough that a cell is a real region rather
/// than a pixel.
pub(crate) const HIZ_DIM: usize = 64;

pub(crate) struct HiZ {
    device: ash::Device,
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    address: vk::DeviceAddress,
    pipeline: vk::Pipeline,
    /// The previous frame's grid, read back on the CPU.
    grid: Vec<f32>,
    /// False until the GPU has written once. The first frame of every render
    /// culls nothing, which is what keeps the golden images honest.
    primed: bool,
}

impl HiZ {
    pub(crate) fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        cache: vk::PipelineCache,
        layout: vk::PipelineLayout,
        names: &DebugNames,
    ) -> Result<Self, RenderError> {
        let bytes = (HIZ_DIM * HIZ_DIM * size_of::<f32>()) as u64;
        let (buffer, mut allocation, address) = create_address_buffer_in(
            device,
            allocator,
            bytes,
            "loom.hiz",
            vk::BufferUsageFlags::STORAGE_BUFFER,
            // Written by the GPU, read by the CPU one frame later.
            MemoryLocation::GpuToCpu,
        )?;
        names.set(buffer, "loom.hiz");
        // **Filled with the far plane before anything reads it.** An unwritten
        // grid is all zeros, which reads as "everything in this cell is at the
        // near plane" and culls the entire scene — measured, and it deleted
        // seven of eight objects. 1.0 is the value that occludes nothing, so a
        // reduction that never ran costs a draw call rather than a picture.
        if let Some(mapped) = allocation.mapped_slice_mut() {
            for chunk in mapped.chunks_exact_mut(4) {
                chunk.copy_from_slice(&1.0_f32.to_le_bytes());
            }
        }

        let module = create_shader_module(device, crate::SCENE_SPV)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(c"hizReduceMain");
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout);
        // SAFETY: the stage's module and layout outlive the call, and the
        // entry point named exists in `SCENE_SPV`.
        let created = unsafe { device.create_compute_pipelines(cache, &[info], None) };
        let pipeline = match created {
            Ok(p) => p[0],
            Err((_, e)) => {
                // SAFETY: neither object has been handed to a command buffer,
                // and the pipeline that would have used them was not created.
                unsafe {
                    device.destroy_shader_module(module, None);
                    device.destroy_buffer(buffer, None);
                }
                let _ = allocator.free(allocation);
                return Err(RenderError::Vulkan(e));
            }
        };
        // SAFETY: the pipeline holds no reference to the module after creation.
        unsafe { device.destroy_shader_module(module, None) };
        names.set(pipeline, "loom.hiz.pipeline");

        Ok(Self {
            device: device.clone(),
            buffer,
            allocation: Some(allocation),
            address,
            pipeline,
            grid: vec![1.0; HIZ_DIM * HIZ_DIM],
            primed: false,
        })
    }

    /// The address the shader writes through, or zero before the first
    /// dispatch — the shader treats null as "skip", which is what makes a
    /// single-frame render leave the grid alone.
    pub(crate) fn address(&self) -> vk::DeviceAddress {
        self.address
    }

    /// Last frame's grid, or `None` until the GPU has written one.
    pub(crate) fn grid(&self) -> Option<&[f32]> {
        // **`LOOM_NO_HIZ=1` turns the occlusion half off and leaves the frustum
        // half alone.** An occlusion cull's failure mode is deleting something
        // visible, and the only way to see that is to render the same frames
        // with and without it and compare. Without a switch that comparison
        // needs two builds.
        if std::env::var_os("LOOM_NO_HIZ").is_some() {
            return None;
        }
        self.primed.then_some(self.grid.as_slice())
    }

    /// Copy the GPU's grid into `self.grid`. Called once per frame, before the
    /// cull that reads it, and it reads whatever the *previous* frame left —
    /// which is the whole design and the reason it never stalls.
    pub(crate) fn read_back(&mut self) {
        // Reported here rather than at the cull, because a grid that is all far
        // plane and a grid that is all near plane look identical downstream and
        // mean opposite things.
        let probe = std::env::var_os("LOOM_CULL_PROBE").is_some();
        let Some(allocation) = self.allocation.as_ref() else {
            return;
        };
        let Some(mapped) = allocation.mapped_slice() else {
            return;
        };
        let want = HIZ_DIM * HIZ_DIM * size_of::<f32>();
        if mapped.len() < want {
            return;
        }
        for (cell, chunk) in self.grid.iter_mut().zip(mapped[..want].chunks_exact(4)) {
            *cell = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        if probe {
            let lo = self.grid.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = self.grid.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            eprintln!("loom: hiz grid depth {lo:.4}..{hi:.4}");
        }
    }

    /// Record the reduction. The depth image must already be readable by a
    /// shader — the graph owns that transition, never-do #4.
    pub(crate) fn record(
        &self,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        sets: &[(u32, vk::DescriptorSet)],
        push: &crate::renderer::Push,
    ) {
        let device = &self.device;
        // SAFETY: the caller has put the depth image in SHADER_READ_ONLY and
        // this pipeline's layout is the one the sets were allocated against.
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            for (index, set) in sets {
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
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
            let groups = (HIZ_DIM / 8) as u32;
            device.cmd_dispatch(cmd, groups, groups, 1);
        }
    }

    /// Called once the frame that recorded a reduction has been submitted.
    /// Separate from `record` because the graph closure holds `&self`, and
    /// because "a dispatch was recorded" and "a grid exists to read" are
    /// different facts a frame apart.
    pub(crate) fn mark_primed(&mut self) {
        self.primed = true;
    }

    /// Give the allocation back, before the allocator goes.
    pub(crate) fn free(&mut self, allocator: &mut Allocator) {
        if let Some(allocation) = self.allocation.take() {
            let _ = allocator.free(allocation);
        }
    }
}

impl Drop for HiZ {
    fn drop(&mut self) {
        // SAFETY: the caller idles the device before teardown. Handles are
        // destroyed unconditionally — ADR 0084's sibling lesson from 70cce8e:
        // destroying a handle never needed the allocator.
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_buffer(self.buffer, None);
        }
    }
}
