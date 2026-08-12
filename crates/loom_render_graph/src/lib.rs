//! A render graph that owns every barrier.
//!
//! `CLAUDE.md` never-do #4: **never place a barrier outside the render graph.**
//! Hand-placing barriers per call site is how projects accumulate subtle,
//! hardware-specific corruption — the kind that works on one driver and fails
//! on another, and that `cargo check` is completely blind to (brief §7.3).
//!
//! The model is deliberately small. Passes run in declaration order; the graph
//! tracks each resource's current layout, stage, and access, and emits exactly
//! the transitions needed before each pass. That is the part that must be
//! automatic. Reordering, culling, and memory aliasing are not, and are not
//! here.
//!
//! `ponytail:` no pass reordering, no dead-pass culling, no transient memory
//! aliasing. Declaration order *is* execution order, and every resource is
//! externally owned. Upgrade path when a frame has enough passes to care —
//! read `caldera` (sjb3d), which has already worked this out in Rust: build a
//! DAG from the declared reads/writes, topologically sort it, then alias
//! transient allocations whose lifetimes do not overlap. [`Pass`]'s declared
//! accesses are already the data a DAG needs, so nothing here has to change
//! shape.

use ash::vk;

/// A resource the graph tracks. Images only for now — buffers reached by
/// device address need no layout transitions, which is most of what this does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageId(pub u32);

/// How a pass touches a resource.
///
/// Each variant carries the layout, pipeline stage, and access mask the driver
/// needs, so a caller declares *intent* and never spells out a barrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Written as a colour attachment.
    ColorWrite,
    /// Written (and tested) as a depth attachment.
    DepthWrite,
    /// Tested against but not written — a forward pass after a depth prepass.
    DepthRead,
    /// Sampled in a shader.
    ShaderRead,
    /// Source of a transfer.
    TransferSrc,
    /// Destination of a transfer.
    TransferDst,
    /// Handed to the presentation engine.
    Present,
}

impl Access {
    /// The layout this access requires.
    #[must_use]
    pub fn layout(self) -> vk::ImageLayout {
        match self {
            Self::ColorWrite => vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            Self::DepthWrite => vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL,
            Self::DepthRead => vk::ImageLayout::DEPTH_READ_ONLY_OPTIMAL,
            Self::ShaderRead => vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            Self::TransferSrc => vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            Self::TransferDst => vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            Self::Present => vk::ImageLayout::PRESENT_SRC_KHR,
        }
    }

    fn stage(self) -> vk::PipelineStageFlags2 {
        match self {
            Self::ColorWrite => vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            Self::DepthWrite | Self::DepthRead => vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            Self::ShaderRead => vk::PipelineStageFlags2::FRAGMENT_SHADER,
            Self::TransferSrc | Self::TransferDst => vk::PipelineStageFlags2::ALL_TRANSFER,
            // Nothing in the pipeline reads it; the presentation engine does.
            Self::Present => vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
        }
    }

    fn access(self) -> vk::AccessFlags2 {
        match self {
            Self::ColorWrite => vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            Self::DepthWrite => vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
                | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ,
            Self::DepthRead => vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ,
            Self::ShaderRead => vk::AccessFlags2::SHADER_SAMPLED_READ,
            Self::TransferSrc => vk::AccessFlags2::TRANSFER_READ,
            Self::TransferDst => vk::AccessFlags2::TRANSFER_WRITE,
            Self::Present => vk::AccessFlags2::empty(),
        }
    }

    fn aspect(self) -> vk::ImageAspectFlags {
        match self {
            Self::DepthWrite | Self::DepthRead => vk::ImageAspectFlags::DEPTH,
            _ => vk::ImageAspectFlags::COLOR,
        }
    }

    /// Whether this access writes. Read-after-read needs no barrier, and
    /// emitting one anyway is a real (if small) cost on every frame.
    fn writes(self) -> bool {
        matches!(
            self,
            Self::ColorWrite | Self::DepthWrite | Self::TransferDst
        )
    }
}

/// A resource's last known state.
#[derive(Debug, Clone, Copy)]
struct State {
    layout: vk::ImageLayout,
    stage: vk::PipelineStageFlags2,
    access: vk::AccessFlags2,
}

impl State {
    /// An image nobody has touched yet.
    const fn undefined() -> Self {
        Self {
            layout: vk::ImageLayout::UNDEFINED,
            stage: vk::PipelineStageFlags2::TOP_OF_PIPE,
            access: vk::AccessFlags2::empty(),
        }
    }
}

/// An image registered with the graph.
struct Registered {
    image: vk::Image,
    state: State,
    name: &'static str,
}

/// GPU timestamps around every pass, resolved after the frame's fence.
///
/// The graph already knows where a pass begins and ends, so this lives here
/// rather than being sprinkled through call sites — the same argument that puts
/// barriers here (never-do #4).
///
/// **Nothing is recorded unless a caller hands one to [`RenderGraph::time`].**
/// Two timestamp writes per pass are commands in the buffer like any other, and
/// an instrument that is always on is an instrument that changes what it
/// measures.
pub struct GpuTimers {
    pool: vk::QueryPool,
    /// Nanoseconds per tick, from `VkPhysicalDeviceLimits::timestampPeriod`.
    /// Never hardcoded: `vulkaninfo` reports 1.0 on this box's NVIDIA driver,
    /// and other vendors report tens of nanoseconds, so a constant would be
    /// silently wrong by an order of magnitude on somebody else's GPU.
    period_ns: f32,
    /// Only the low `timestampValidBits` of a written value are meaningful.
    mask: u64,
    /// Passes the pool can hold timestamps for.
    capacity: u32,
    /// Filled by [`RenderGraph::execute`], read by [`Self::resolve`].
    pending: Vec<&'static str>,
    times: Vec<(String, f64)>,
}

/// Milliseconds between two raw timestamp values.
///
/// Split out because it is the only arithmetic here that can be wrong in a way
/// no GPU is needed to see: the mask, and the counter wrapping past it.
fn elapsed_ms(start: u64, end: u64, mask: u64, period_ns: f32) -> f64 {
    let (start, end) = (start & mask, end & mask);
    // The counter is `timestampValidBits` wide and wraps. Wrapping is rare
    // (2^64 ticks is centuries at 1ns) but wrong-by-a-universe when ignored.
    let ticks = end.wrapping_sub(start) & mask;
    #[allow(clippy::cast_precision_loss)]
    let ticks = ticks as f64;
    ticks * f64::from(period_ns) / 1_000_000.0
}

impl GpuTimers {
    /// Build a pool sized for `passes` passes, or `None` when this queue family
    /// cannot write timestamps at all.
    ///
    /// `valid_bits` is the queue family's `timestampValidBits`. Zero means the
    /// family reports no usable timestamps — the honest answer there is "no
    /// numbers", not numbers made of whatever the driver left in the buffer.
    ///
    /// # Errors
    /// The driver's error if `vkCreateQueryPool` fails.
    pub fn new(
        device: &ash::Device,
        period_ns: f32,
        valid_bits: u32,
        passes: u32,
    ) -> Result<Option<Self>, vk::Result> {
        if valid_bits == 0 || period_ns <= 0.0 {
            return Ok(None);
        }
        let info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(passes * 2);
        // SAFETY: `info` is fully initialised and outlives the call.
        let pool = unsafe { device.create_query_pool(&info, None) }?;
        Ok(Some(Self {
            pool,
            period_ns,
            mask: if valid_bits >= 64 {
                u64::MAX
            } else {
                (1_u64 << valid_bits) - 1
            },
            capacity: passes,
            pending: Vec::new(),
            times: Vec::new(),
        }))
    }

    /// The pool handle, so the owner can name it via `VK_EXT_debug_utils`.
    #[must_use]
    pub fn pool(&self) -> vk::QueryPool {
        self.pool
    }

    /// Read back the last executed frame's timings. Call **after** waiting on
    /// that frame's fence; the results are guaranteed available by then.
    pub fn resolve(&mut self, device: &ash::Device) {
        self.times.clear();
        if self.pending.is_empty() {
            return;
        }
        let mut raw = vec![0_u64; self.pending.len() * 2];
        // SAFETY: every query in the range was reset and written by the
        // command buffer whose fence the caller has waited on.
        let read = unsafe {
            device.get_query_pool_results(
                self.pool,
                0,
                &mut raw,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )
        };
        if read.is_err() {
            return;
        }
        for (index, name) in self.pending.iter().enumerate() {
            let ms = elapsed_ms(raw[index * 2], raw[index * 2 + 1], self.mask, self.period_ns);
            self.times.push(((*name).to_string(), ms));
        }
    }

    /// Per-pass milliseconds from the last [`Self::resolve`], in pass order.
    #[must_use]
    pub fn times(&self) -> &[(String, f64)] {
        &self.times
    }

    /// Destroy the pool.
    ///
    /// # Safety
    /// No command buffer referencing this pool may still be in flight.
    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: the caller guarantees nothing is in flight.
        unsafe { device.destroy_query_pool(self.pool, None) };
        self.pool = vk::QueryPool::null();
        self.pending.clear();
    }
}

/// What a pass does once its resources are in the right layouts.
type Record<'a> = Box<dyn FnOnce(&ash::Device, vk::CommandBuffer) + 'a>;

/// One unit of work, with the resources it touches declared up front.
pub struct Pass<'a> {
    name: &'static str,
    accesses: Vec<(ImageId, Access)>,
    record: Record<'a>,
}

/// Declares passes, then executes them with barriers inserted automatically.
#[derive(Default)]
pub struct RenderGraph<'a> {
    images: Vec<Registered>,
    passes: Vec<Pass<'a>>,
    timers: Option<&'a mut GpuTimers>,
}

impl<'a> RenderGraph<'a> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            images: Vec::new(),
            passes: Vec::new(),
            timers: None,
        }
    }

    /// Time every pass on the GPU. The timings land in `timers` when
    /// [`GpuTimers::resolve`] is called after this frame's fence.
    pub fn time(&mut self, timers: &'a mut GpuTimers) {
        self.timers = Some(timers);
    }

    /// Register an image whose current state is unknown.
    ///
    /// `UNDEFINED` is the honest starting point for a target that is cleared
    /// every frame: the contents are genuinely not preserved, and claiming
    /// otherwise would make the driver do pointless work keeping them.
    pub fn import(&mut self, name: &'static str, image: vk::Image) -> ImageId {
        self.import_with_layout(name, image, vk::ImageLayout::UNDEFINED)
    }

    /// Register an image whose current layout is known.
    pub fn import_with_layout(
        &mut self,
        name: &'static str,
        image: vk::Image,
        layout: vk::ImageLayout,
    ) -> ImageId {
        let id = ImageId(u32::try_from(self.images.len()).unwrap_or(u32::MAX));
        self.images.push(Registered {
            image,
            state: State {
                layout,
                ..State::undefined()
            },
            name,
        });
        id
    }

    /// Add a pass. `accesses` is the complete list of resources it touches —
    /// anything omitted will not be transitioned, which is the one way to get
    /// this wrong, and it is exactly what synchronization validation catches.
    pub fn pass(
        &mut self,
        name: &'static str,
        accesses: &[(ImageId, Access)],
        record: impl FnOnce(&ash::Device, vk::CommandBuffer) + 'a,
    ) {
        self.passes.push(Pass {
            name,
            accesses: accesses.to_vec(),
            record: Box::new(record),
        });
    }

    /// Decide what a single access needs, and advance the resource's state.
    ///
    /// The **only** place the barrier decision is made. `execute` records what
    /// this returns; [`Self::plan`] returns it without a device. Two copies of
    /// this logic would let the recorded barriers and the tested barriers drift
    /// apart, which is the failure mode a test is supposed to prevent.
    fn decide(&mut self, pass: &'static str, id: ImageId, access: Access) -> Option<Decision> {
        let resource = self.images.get_mut(id.0 as usize)?;
        let target = State {
            layout: access.layout(),
            stage: access.stage(),
            access: access.access(),
        };

        // A barrier is needed when the layout changes, or when either side
        // writes. Read-after-read in the same layout is already ordered.
        let layout_changed = resource.state.layout != target.layout;
        let hazard = access.writes()
            || resource.state.access.intersects(
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
                    | vk::AccessFlags2::TRANSFER_WRITE,
            );
        if !layout_changed && !hazard {
            return None;
        }

        let barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(resource.state.stage)
            .src_access_mask(resource.state.access)
            .dst_stage_mask(target.stage)
            .dst_access_mask(target.access)
            .old_layout(resource.state.layout)
            .new_layout(target.layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(resource.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(access.aspect())
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );

        let transition = Transition {
            pass,
            image: resource.name,
            from: resource.state.layout,
            to: target.layout,
        };
        resource.state = target;
        Some((transition, barrier))
    }

    /// The transitions this graph would emit, without touching a device.
    ///
    /// Consumes the passes, exactly as `execute` does, so the two cannot
    /// disagree about what a graph decides.
    #[must_use]
    pub fn plan(mut self) -> Vec<Transition> {
        let mut emitted = Vec::new();
        let passes = std::mem::take(&mut self.passes);
        for pass in &passes {
            for (id, access) in &pass.accesses {
                if let Some((transition, _)) = self.decide(pass.name, *id, *access) {
                    emitted.push(transition);
                }
            }
        }
        emitted
    }

    /// Record every pass into `cmd`, emitting transitions between them.
    ///
    /// Returns the barriers emitted, which the tests assert on — a graph that
    /// silently emits none is indistinguishable from a graph that works, right
    /// up until it corrupts something.
    pub fn execute(mut self, device: &ash::Device, cmd: vk::CommandBuffer) -> Vec<Transition> {
        let mut emitted = Vec::new();
        let passes = std::mem::take(&mut self.passes);

        // **Reset before anything is written, every frame.** A query that was
        // not reset since its last write returns stale data or none at all,
        // and it is the single most common way this feature is built wrong.
        // Recorded here, at the top of the command buffer, so it is outside
        // any dynamic-rendering block — a reset inside one is invalid.
        let mut timers = self.timers.take();
        if let Some(t) = timers.as_deref_mut() {
            let timed = passes.len().min(t.capacity as usize);
            t.pending = passes.iter().take(timed).map(|p| p.name).collect();
            // SAFETY: the range is within the pool's query count, and the
            // caller has waited on the fence of the frame that last used it.
            unsafe {
                device.cmd_reset_query_pool(
                    cmd,
                    t.pool,
                    0,
                    u32::try_from(timed).unwrap_or(0) * 2,
                );
            }
        }

        for (index, pass) in passes.into_iter().enumerate() {
            // The opening timestamp goes *before* the barriers, so a pass owns
            // the cost of the transitions it required. ALL_COMMANDS on both
            // ends means "when everything before this point has completed",
            // which is the only reading that does not smear neighbouring
            // passes into each other — at the price of some lost overlap
            // between them, so a timed frame is slightly slower than an
            // untimed one and each pass reads slightly pessimistically.
            if let Some(t) = timers.as_deref_mut().filter(|t| index < t.pending.len()) {
                // SAFETY: query index is inside the reset range.
                unsafe {
                    device.cmd_write_timestamp2(
                        cmd,
                        vk::PipelineStageFlags2::ALL_COMMANDS,
                        t.pool,
                        u32::try_from(index).unwrap_or(0) * 2,
                    );
                }
            }
            let mut barriers = Vec::new();
            for (id, access) in &pass.accesses {
                if let Some((transition, barrier)) = self.decide(pass.name, *id, *access) {
                    emitted.push(transition);
                    barriers.push(barrier);
                }
            }

            if !barriers.is_empty() {
                let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
                // SAFETY: `barriers` outlives the call and every image is live
                // for the caller-provided command buffer's lifetime.
                unsafe { device.cmd_pipeline_barrier2(cmd, &dependency) };
            }

            (pass.record)(device, cmd);

            if let Some(t) = timers.as_deref_mut().filter(|t| index < t.pending.len()) {
                // SAFETY: query index is inside the reset range.
                unsafe {
                    device.cmd_write_timestamp2(
                        cmd,
                        vk::PipelineStageFlags2::ALL_COMMANDS,
                        t.pool,
                        u32::try_from(index).unwrap_or(0) * 2 + 1,
                    );
                }
            }
        }

        emitted
    }
}

/// What [`RenderGraph::decide`] concluded: the transition to report, and the
/// barrier that realises it.
type Decision = (Transition, vk::ImageMemoryBarrier2<'static>);

/// A layout transition the graph decided to emit. Returned for tests and for
/// debugging; a graph is much easier to trust when it can say what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub pass: &'static str,
    pub image: &'static str,
    pub from: vk::ImageLayout,
    pub to: vk::ImageLayout,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Barrier decisions are testable without a GPU: what the graph chooses to
    /// emit is pure logic over declared accesses. This calls the **same**
    /// `decide` that `execute` records with — an earlier version of this helper
    /// re-implemented the logic, which would have let the two drift silently.
    /// Whether the driver agrees is what synchronization validation checks, in
    /// the render tests.
    fn plan(build: impl FnOnce(&mut RenderGraph<'_>)) -> Vec<Transition> {
        let mut graph = RenderGraph::new();
        build(&mut graph);
        graph.plan()
    }

    #[test]
    fn a_freshly_imported_image_transitions_out_of_undefined() {
        let transitions = plan(|g| {
            let color = g.import("color", vk::Image::null());
            g.pass("forward", &[(color, Access::ColorWrite)], |_, _| {});
        });

        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].from, vk::ImageLayout::UNDEFINED);
        assert_eq!(
            transitions[0].to,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );
    }

    /// The three-pass shape from brief §6 M4: depth prepass, forward, post.
    #[test]
    fn a_three_pass_graph_transitions_each_resource_as_its_use_changes() {
        let transitions = plan(|g| {
            let depth = g.import("depth", vk::Image::null());
            let hdr = g.import("hdr", vk::Image::null());
            let ldr = g.import("ldr", vk::Image::null());

            g.pass("depth_prepass", &[(depth, Access::DepthWrite)], |_, _| {});
            g.pass(
                "forward",
                &[(depth, Access::DepthRead), (hdr, Access::ColorWrite)],
                |_, _| {},
            );
            g.pass(
                "post",
                &[(hdr, Access::ShaderRead), (ldr, Access::ColorWrite)],
                |_, _| {},
            );
        });

        let summary: Vec<(&str, &str)> = transitions
            .iter()
            .map(|t| (t.pass, t.image))
            .collect();
        assert_eq!(
            summary,
            [
                ("depth_prepass", "depth"),
                // Depth flips to read-only for the forward pass...
                ("forward", "depth"),
                ("forward", "hdr"),
                // ...and the HDR target becomes a sampled texture for post.
                ("post", "hdr"),
                ("post", "ldr"),
            ]
        );
    }

    /// Two reads in a row need no barrier between them. Emitting one is not
    /// incorrect, just wasteful — and on a per-frame path waste compounds.
    #[test]
    fn read_after_read_emits_nothing() {
        let transitions = plan(|g| {
            let tex = g.import("tex", vk::Image::null());
            g.pass("a", &[(tex, Access::ShaderRead)], |_, _| {});
            g.pass("b", &[(tex, Access::ShaderRead)], |_, _| {});
        });

        assert_eq!(transitions.len(), 1, "only the initial transition");
        assert_eq!(transitions[0].pass, "a");
    }

    /// Write-after-write in the same layout still needs a barrier: the second
    /// write must not race the first. Missing this is the classic corruption
    /// bug, and it is invisible without synchronization validation.
    #[test]
    fn write_after_write_still_emits_a_barrier() {
        let transitions = plan(|g| {
            let color = g.import("color", vk::Image::null());
            g.pass("first", &[(color, Access::ColorWrite)], |_, _| {});
            g.pass("second", &[(color, Access::ColorWrite)], |_, _| {});
        });

        assert_eq!(transitions.len(), 2, "same layout, but still a hazard");
        assert_eq!(transitions[1].pass, "second");
        assert_eq!(transitions[1].from, transitions[1].to);
    }

    /// The one part of GPU timing that can be wrong without a GPU present.
    /// A 1ns period is this box's driver; 40ns is a plausible other one, and
    /// hardcoding either is how a timing report ends up wrong by 40x.
    #[test]
    fn timestamp_ticks_convert_with_the_device_period_and_mask() {
        let mask = (1_u64 << 36) - 1;
        // A million ticks at 1ns each is a millisecond.
        assert!((elapsed_ms(0, 1_000_000, mask, 1.0) - 1.0).abs() < 1e-9);
        assert!((elapsed_ms(0, 1_000_000, mask, 40.0) - 40.0).abs() < 1e-9);
        // High bits beyond timestampValidBits are garbage and must be dropped,
        // not read as an enormous elapsed time.
        assert!((elapsed_ms(1 << 40, (1 << 40) + 1_000_000, mask, 1.0) - 1.0).abs() < 1e-9);
        // And the counter wraps at that width rather than going negative.
        assert!((elapsed_ms(mask - 999, mask + 1, mask, 1.0) - 0.001).abs() < 1e-9);
    }

    #[test]
    fn present_is_the_last_transition() {
        let transitions = plan(|g| {
            let swap = g.import("swapchain", vk::Image::null());
            g.pass("forward", &[(swap, Access::ColorWrite)], |_, _| {});
            g.pass("present", &[(swap, Access::Present)], |_, _| {});
        });

        assert_eq!(transitions.last().unwrap().to, vk::ImageLayout::PRESENT_SRC_KHR);
    }
}
