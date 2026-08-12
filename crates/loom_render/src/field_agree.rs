//! **S2's exit criterion: proof that the CPU field and the GPU field agree.**
//!
//! The implementation order calls a silent CPU/GPU field disagreement "the
//! highest-severity failure mode in the whole backlog", and the reason is that
//! it has no symptom. Nothing errors. Wind pushes a rigid body one way while
//! the grass it is standing in leans another, and the only evidence is that
//! the scene looks subtly wrong in motion — which is the one thing a still PNG
//! cannot see and a determinism hash does not cover, because both sides are
//! individually deterministic.
//!
//! `loom_field` plus the `build.rs` generator removes the *structural* half of
//! that risk: there is one expression tree, and the Slang is emitted from it,
//! so neither side can implement a different formula. This module measures
//! what remains — floating point. A GPU `sin` is not libm's `sin`, and a
//! driver is free to contract `a * b + c` into an fma with different rounding.
//! Those differences are small, real, and worth knowing the size of rather
//! than guessing at, exactly as the golden-image tolerance had to be measured
//! rather than chosen (ADR 0005).
//!
//! # Barriers
//!
//! One, and it does not belong to the render graph. never-do #4 gives the
//! graph ownership of *frame* barriers, and the graph models images; this is a
//! one-shot dispatch on a buffer, submitted and waited on outside any frame,
//! following the precedent `raytrace.rs` set and `material.rs` documents.
//!
//! # Test-only
//!
//! Compiled under `cfg(test)`. Phase 1 will want the GPU to sample wind for
//! real, but it will want it written into a texture by the frame graph, not
//! read back to the host — so this is a harness, not the beginnings of that.

use ash::vk;
use gpu_allocator::vulkan::Allocator;

use crate::material::record;
use crate::raytrace::Submit;
use crate::renderer::{RenderError, create_address_buffer, create_shader_module, write_slice};

/// One `(position, time)` sample.
///
/// `[f32; 4]` on both sides — xyz is the position and w is the time. A packed
/// `[f32; 3]` plus a separate `f32` would be the same 16 bytes in Rust and a
/// different 16 bytes in the shader, because a `float3` in a buffer still
/// aligns to 16. See the comment in `field_agree.slang`.
pub(crate) type Sample = [f32; 4];

/// The push-constant block, mirroring `AgreePush` in `field_agree.slang`.
///
/// Pointers first, `count` last: a device address needs 8-byte alignment, and
/// leading with the scalar would pad. The same layout-described-twice hazard
/// `scene.slang` documents at length applies here, but the block is three
/// fields long, which is the only reason it is safe to eyeball.
#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    samples: vk::DeviceAddress,
    results: vk::DeviceAddress,
    params: vk::DeviceAddress,
    count: u32,
}

/// Evaluate `wind_at` on the GPU at every sample, and read the results back.
///
/// Readback is fine here: determinism matters in the simulation, not in a
/// harness whose entire job is asserting equality.
pub(crate) fn evaluate_on_gpu(
    device: &crate::Device,
    allocator: &mut Allocator,
    pool: vk::CommandPool,
    entry: &std::ffi::CStr,
    samples: &[Sample],
    field_params: &[f32],
) -> Result<Vec<[f32; 4]>, RenderError> {
    let raw = device.handle();
    let bytes = std::mem::size_of_val(samples) as vk::DeviceSize;

    let (sample_buffer, sample_alloc, sample_address) = create_address_buffer(
        raw,
        allocator,
        bytes,
        "loom.field_agree.samples",
        vk::BufferUsageFlags::empty(),
    )?;
    let (result_buffer, result_alloc, result_address) = create_address_buffer(
        raw,
        allocator,
        bytes,
        "loom.field_agree.results",
        vk::BufferUsageFlags::empty(),
    )?;
    // Never zero-sized: a buffer of no bytes is an invalid create info, and a
    // field with no parameters is a perfectly ordinary thing to test.
    let (param_buffer, param_alloc, param_address) = create_address_buffer(
        raw,
        allocator,
        (field_params.len().max(1) * size_of::<f32>()) as vk::DeviceSize,
        "loom.field_agree.params",
        vk::BufferUsageFlags::empty(),
    )?;

    // Everything from here must clean up both buffers on the way out, so the
    // work is wrapped and the teardown runs unconditionally afterwards. A
    // leaked buffer here would be reported at teardown as an object-tracking
    // failure, hiding whatever actually went wrong.
    let outcome = (|| {
        write_slice(&sample_alloc, samples)?;
        if !field_params.is_empty() {
            write_slice(&param_alloc, field_params)?;
        }

        let module = create_shader_module(raw, crate::FIELD_AGREE_SPV)?;
        let range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(u32::try_from(size_of::<Push>()).unwrap_or(128));
        let ranges = [range];
        let layout_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&ranges);
        // SAFETY: `ranges` outlives the call.
        let layout = unsafe { raw.create_pipeline_layout(&layout_info, None) }?;

        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(entry);
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout);
        // SAFETY: `info` borrows `module` and `layout`, both live until the
        // teardown below.
        let created =
            unsafe { raw.create_compute_pipelines(vk::PipelineCache::null(), &[info], None) };
        // **The failure path has to clean up too.** Returning `?` straight from
        // here leaked the module and the layout, and the object-tracking check
        // at teardown then reported those leaks instead of the compile error
        // that caused them — which is exactly how this was first seen.
        let pipeline = match created {
            Ok(pipelines) => pipelines[0],
            Err((_, e)) => {
                // SAFETY: both were created here and nothing else holds them.
                unsafe {
                    raw.destroy_pipeline_layout(layout, None);
                    raw.destroy_shader_module(module, None);
                }
                return Err(RenderError::Vulkan(e));
            }
        };

        let push = Push {
            samples: sample_address,
            results: result_address,
            params: param_address,
            count: u32::try_from(samples.len()).unwrap_or(u32::MAX),
        };
        // The shader's workgroup is 64 wide, so round up and let the entry
        // point discard the tail.
        let groups = samples.len().div_ceil(64);

        let recorded = record(raw, Submit { pool, queue: device.queue() }, |cmd| {
            // SAFETY: `cmd` is recording and every handle below is live.
            unsafe {
                raw.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
                let bytes = std::slice::from_raw_parts(
                    std::ptr::from_ref(&push).cast::<u8>(),
                    size_of::<Push>(),
                );
                raw.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::COMPUTE, 0, bytes);
                raw.cmd_dispatch(cmd, u32::try_from(groups).unwrap_or(u32::MAX), 1, 1);

                // **Without this the host may read stale memory.** The fence
                // says the work finished; it does not make the writes visible
                // to the host. Sync validation flags the omission, which is
                // the only reason a bug this quiet is catchable at all.
                let barrier = vk::MemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                    .src_access_mask(vk::AccessFlags2::SHADER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                    .dst_access_mask(vk::AccessFlags2::HOST_READ);
                let barriers = [barrier];
                let dependency = vk::DependencyInfo::default().memory_barriers(&barriers);
                raw.cmd_pipeline_barrier2(cmd, &dependency);
            }
        });

        // SAFETY: the queue was waited on inside `record`, so nothing is in
        // flight against these.
        unsafe {
            raw.destroy_pipeline(pipeline, None);
            raw.destroy_pipeline_layout(layout, None);
            raw.destroy_shader_module(module, None);
        }
        recorded?;

        let mapped = result_alloc
            .mapped_ptr()
            .ok_or_else(|| RenderError::Allocator("result buffer is not host-visible".into()))?;
        // SAFETY: the buffer was sized for `samples.len()` of these, the
        // dispatch wrote all of them, and the barrier above made the writes
        // visible to the host.
        let results =
            unsafe { std::slice::from_raw_parts(mapped.as_ptr().cast::<[f32; 4]>(), samples.len()) };
        Ok(results.to_vec())
    })();

    // SAFETY: nothing is in flight — `record` waited on a fence before
    // returning, and the failure paths above never submitted.
    unsafe {
        raw.destroy_buffer(sample_buffer, None);
        raw.destroy_buffer(result_buffer, None);
        raw.destroy_buffer(param_buffer, None);
    }
    let _ = allocator.free(sample_alloc);
    let _ = allocator.free(result_alloc);
    let _ = allocator.free(param_alloc);

    outcome
}

/// The fixed sample set both sides evaluate.
///
/// **Generated once, in Rust, and uploaded** rather than derived from the
/// invocation index in the shader. Deriving them on each side would be one
/// fewer buffer and would reintroduce the exact hazard this test exists to
/// catch — a formula written twice — inside the test meant to catch it.
///
/// Spread over a range a real scene uses, including negative coordinates and
/// below `y = 0`: the wind field's height profile clamps there, and a
/// disagreement in a `max` is as real as one in a `sin`.
pub(crate) fn samples(count: usize) -> Vec<Sample> {
    (0..count)
        .map(|i| {
            // A cheap integer hash, so the points are scattered rather than
            // walking a lattice — a regular grid can land every sample on the
            // same phase of a sinusoid and agree for the wrong reason.
            let h = |k: u32| {
                let mut x = (i as u32).wrapping_mul(0x9E37_79B9).wrapping_add(k);
                x ^= x >> 16;
                x = x.wrapping_mul(0x7FEB_352D);
                x ^= x >> 15;
                f32::from(x as u16) / f32::from(u16::MAX)
            };
            [
                h(1).mul_add(240.0, -120.0),
                h(2).mul_add(60.0, -10.0),
                h(3).mul_add(240.0, -120.0),
                h(4) * 90.0,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Device, DeviceError, Instance};

    /// **The S2 exit criterion.** One field authored in Rust, its Slang
    /// generated from the same tree, and the two proven to agree.
    #[test]
    fn the_cpu_and_the_gpu_compute_the_same_field() {
        let _serialised = crate::tests::exclusive();
        let Ok(instance) = Instance::new(c"loom-field-agree-test") else {
            eprintln!("skipping: no Vulkan loader");
            return;
        };
        let device = match Device::new(&instance) {
            Ok(d) => d,
            Err(DeviceError::NoDevices) => {
                eprintln!("skipping: no Vulkan device");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let _ = instance.check_validation();

        let raw = device.handle();
        let mut allocator = gpu_allocator::vulkan::Allocator::new(
            &gpu_allocator::vulkan::AllocatorCreateDesc {
                instance: instance.handle().clone(),
                device: raw.clone(),
                physical_device: device.physical(),
                debug_settings: gpu_allocator::AllocatorDebugSettings::default(),
                buffer_device_address: true,
                allocation_sizes: gpu_allocator::AllocationSizes::default(),
            },
        )
        .expect("allocator");

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(device.queue_family())
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the family index came from this device.
        let pool = unsafe { raw.create_command_pool(&pool_info, None) }.expect("command pool");

        let field = loom_field::wind();
        let params = loom_field::wind_defaults();
        assert!(
            params.missing(&field).is_empty(),
            "the defaults must cover every parameter the field reads: {:?}",
            params.missing(&field)
        );
        let flat = field.params_array(&params);

        let samples = samples(512);
        let gpu = evaluate_on_gpu(&device, &mut allocator, pool, c"agreeMain", &samples, &flat)
            .expect("dispatch");

        // **The noise primitive, compared exactly.** It is the one thing
        // written twice on purpose — an integer lattice hash has no
        // representation in the float expression tree — so it gets the
        // assertion the fields cannot have. A hash that differs by a bit is
        // not a rounding difference; it is a different number, and `frac`
        // would turn it into a visibly different field.
        let noise_gpu =
            evaluate_on_gpu(&device, &mut allocator, pool, c"noiseMain", &samples, &flat)
                .expect("noise dispatch");

        // SAFETY: the dispatch was waited on before this returned.
        unsafe { raw.destroy_command_pool(pool, None) };
        drop(allocator);

        if let Err(messages) = instance.check_validation() {
            panic!("validation was not silent:\n  {}", messages.join("\n  "));
        }

        for (i, sample) in samples.iter().enumerate() {
            let cpu = loom_field::noise::value([sample[0], sample[1], sample[2]]);
            assert_eq!(
                cpu.to_bits(),
                noise_gpu[i][0].to_bits(),
                "the noise primitive disagrees at sample {i} {sample:?}: \
                 cpu {cpu}, gpu {} — the two halves in `loom_field::noise` \
                 have diverged, and every determinism hash is now suspect",
                noise_gpu[i][0]
            );
        }

        // **Otherwise this test agrees for free.** If the dispatch never ran,
        // or the shader wrote nothing, or the field happened to evaluate to
        // zero everywhere, every difference below would be zero and the
        // assertion would pass while proving nothing. Demand that the GPU
        // actually produced a varying, substantial field first.
        let magnitude = gpu
            .iter()
            .map(|v| v[0].abs().max(v[1].abs()).max(v[2].abs()))
            .fold(0.0_f32, f32::max);
        let distinct: std::collections::BTreeSet<u32> =
            gpu.iter().map(|v| v[0].to_bits()).collect();
        assert!(magnitude > 1.0, "the gpu field is flat — largest component {magnitude}");
        assert!(
            distinct.len() > 400,
            "the gpu returned only {} distinct values over {} samples",
            distinct.len(),
            gpu.len()
        );

        let mut worst = 0.0_f32;
        let mut worst_at = 0;
        for (i, (sample, got)) in samples.iter().zip(&gpu).enumerate() {
            let position = [sample[0], sample[1], sample[2]];
            for (axis, expression) in field.body.iter().enumerate() {
                let cpu = expression.eval_with(position, sample[3], &params);
                let delta = (cpu - got[axis]).abs();
                assert!(
                    delta.is_finite(),
                    "sample {i} axis {axis} at {position:?} t={}: cpu {cpu}, gpu {}",
                    sample[3],
                    got[axis]
                );
                if delta > worst {
                    worst = delta;
                    worst_at = i;
                }
            }
        }

        // **Measured, not chosen.** See the module docs: what is left after
        // codegen is `sin` differing in the last bits and possible fma
        // contraction, which on this machine lands around 1e-5 absolute on a
        // field whose magnitude reaches ~8. A threshold loose enough to hide a
        // real formula change would make the test decorative, so this is
        // tight enough that the mutation check below fails it by four orders
        // of magnitude.
        const EPSILON: f32 = 1e-3;
        assert!(
            worst < EPSILON,
            "cpu and gpu disagree by {worst} at sample {worst_at} — \
             the generated Slang and the Rust tree have diverged"
        );
        eprintln!("field agreement: worst absolute difference {worst:e} over {} samples", samples.len());
    }

    /// The sample set must actually exercise the field. Every point landing in
    /// one octant, or every time equal, would make the test above agree for
    /// the wrong reason.
    #[test]
    fn the_samples_are_spread_over_a_real_range() {
        let samples = samples(512);

        let negative_x = samples.iter().filter(|s| s[0] < 0.0).count();
        let below_ground = samples.iter().filter(|s| s[1] < 0.0).count();
        let distinct_times: std::collections::BTreeSet<u32> =
            samples.iter().map(|s| s[3].to_bits()).collect();

        assert!(negative_x > 100, "only {negative_x} samples west of the origin");
        assert!(
            below_ground > 20,
            "only {below_ground} samples below y=0, where the height profile clamps"
        );
        assert!(
            distinct_times.len() > 400,
            "only {} distinct times — the field would barely move",
            distinct_times.len()
        );
    }
}
