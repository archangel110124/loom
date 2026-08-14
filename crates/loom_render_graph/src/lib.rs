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

/// A resource the graph tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageId(pub u32);

/// A buffer the graph tracks.
///
/// **A buffer has no layout, and that was the reason this used to be images
/// only** — "buffers reached by device address need no layout transitions,
/// which is most of what this does". True, and it left the other part
/// unhandled: a compute pass that writes a buffer and a draw that reads it
/// still need an *execution and memory* dependency, or the draw reads whatever
/// was there last frame. That is invisible on this driver about nine frames in
/// ten, which is the worst possible failure profile and exactly the class
/// never-do #4 exists to keep out of call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferId(pub u32);

/// How a pass touches a buffer.
///
/// Deliberately fewer variants than [`Access`]: without layouts, all a buffer
/// barrier carries is a stage and an access mask, and every one of these is a
/// pair the rain path actually uses. Add a variant when a pass needs one, not
/// before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferAccess {
    /// Read and written by a compute shader — the drop simulation's own state.
    ComputeReadWrite,
    /// Read by a compute shader and not written.
    ComputeRead,
    /// Read by a vertex shader — a drop buffer feeding the streak draw.
    VertexRead,
    /// Read by the command processor as `VkDrawIndirectCommand`.
    IndirectRead,
    /// Destination of a transfer — a staging copy or a `vkCmdFillBuffer`.
    TransferDst,
}

impl BufferAccess {
    fn stage(self) -> vk::PipelineStageFlags2 {
        match self {
            Self::ComputeReadWrite | Self::ComputeRead => vk::PipelineStageFlags2::COMPUTE_SHADER,
            Self::VertexRead => vk::PipelineStageFlags2::VERTEX_SHADER,
            Self::IndirectRead => vk::PipelineStageFlags2::DRAW_INDIRECT,
            Self::TransferDst => vk::PipelineStageFlags2::ALL_TRANSFER,
        }
    }

    fn access(self) -> vk::AccessFlags2 {
        match self {
            Self::ComputeReadWrite => {
                vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE
            }
            Self::ComputeRead | Self::VertexRead => vk::AccessFlags2::SHADER_STORAGE_READ,
            Self::IndirectRead => vk::AccessFlags2::INDIRECT_COMMAND_READ,
            Self::TransferDst => vk::AccessFlags2::TRANSFER_WRITE,
        }
    }

    /// Whether this access writes. Read-after-read needs no barrier.
    fn writes(self) -> bool {
        matches!(self, Self::ComputeReadWrite | Self::TransferDst)
    }
}

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
    /// Written as the single-sample destination of a **depth resolve**.
    ///
    /// The layout is the depth attachment's, but the stage and access are the
    /// *colour* ones — which looks wrong and is not. A resolve is modelled as
    /// happening at `COLOR_ATTACHMENT_OUTPUT` with `COLOR_ATTACHMENT_WRITE`
    /// whatever it resolves, and declaring this as an ordinary `DepthWrite`
    /// makes sync validation report a WRITE_AFTER_WRITE against the graph's own
    /// layout transition. It said so plainly the first time; this is the fix.
    DepthResolve,
    /// A *depth* image sampled in a shader.
    ///
    /// Separate from [`Self::ShaderRead`] for one reason and it is not
    /// cosmetic: the barrier's aspect mask has to be `DEPTH`, and a colour
    /// aspect on a depth image is a validation error rather than a wrong
    /// picture. The layout is the same `SHADER_READ_ONLY_OPTIMAL`.
    DepthSample,
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
            Self::DepthWrite | Self::DepthResolve => vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL,
            Self::DepthRead => vk::ImageLayout::DEPTH_READ_ONLY_OPTIMAL,
            Self::ShaderRead | Self::DepthSample => vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            Self::TransferSrc => vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            Self::TransferDst => vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            Self::Present => vk::ImageLayout::PRESENT_SRC_KHR,
        }
    }

    fn stage(self) -> vk::PipelineStageFlags2 {
        match self {
            Self::ColorWrite | Self::DepthResolve => vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            Self::DepthWrite | Self::DepthRead => vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            Self::ShaderRead | Self::DepthSample => vk::PipelineStageFlags2::FRAGMENT_SHADER,
            Self::TransferSrc | Self::TransferDst => vk::PipelineStageFlags2::ALL_TRANSFER,
            // Nothing in the pipeline reads it; the presentation engine does.
            Self::Present => vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
        }
    }

    fn access(self) -> vk::AccessFlags2 {
        match self {
            Self::ColorWrite | Self::DepthResolve => vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            Self::DepthWrite => vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
                | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ,
            Self::DepthRead => vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ,
            Self::ShaderRead | Self::DepthSample => vk::AccessFlags2::SHADER_SAMPLED_READ,
            Self::TransferSrc => vk::AccessFlags2::TRANSFER_READ,
            Self::TransferDst => vk::AccessFlags2::TRANSFER_WRITE,
            Self::Present => vk::AccessFlags2::empty(),
        }
    }

    fn aspect(self) -> vk::ImageAspectFlags {
        match self {
            Self::DepthWrite | Self::DepthRead | Self::DepthSample | Self::DepthResolve => {
                vk::ImageAspectFlags::DEPTH
            }
            _ => vk::ImageAspectFlags::COLOR,
        }
    }

    /// Whether this access writes. Read-after-read needs no barrier, and
    /// emitting one anyway is a real (if small) cost on every frame.
    fn writes(self) -> bool {
        matches!(
            self,
            Self::ColorWrite | Self::DepthWrite | Self::DepthResolve | Self::TransferDst
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
    buffers: Vec<(BufferId, BufferAccess)>,
    record: Record<'a>,
}

/// A buffer's last known state. No layout — only who touched it and how.
#[derive(Debug, Clone, Copy)]
struct BufferState {
    stage: vk::PipelineStageFlags2,
    access: vk::AccessFlags2,
}

impl BufferState {
    /// A buffer nothing in this command buffer has touched.
    const fn untouched() -> Self {
        Self {
            stage: vk::PipelineStageFlags2::TOP_OF_PIPE,
            access: vk::AccessFlags2::empty(),
        }
    }
}

struct RegisteredBuffer {
    buffer: vk::Buffer,
    state: BufferState,
    name: &'static str,
}

/// Declares passes, then executes them with barriers inserted automatically.
#[derive(Default)]
pub struct RenderGraph<'a> {
    images: Vec<Registered>,
    buffers: Vec<RegisteredBuffer>,
    passes: Vec<Pass<'a>>,
    timers: Option<&'a mut GpuTimers>,
}

impl<'a> RenderGraph<'a> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            images: Vec::new(),
            buffers: Vec::new(),
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
        self.pass_with(name, accesses, &[], record);
    }

    /// Register a buffer whose current state is unknown.
    ///
    /// "Unknown" is honest at the top of a frame: whatever touched it last was
    /// in a submission this one has already waited on, so the only dependency
    /// left is within this command buffer. The first access therefore needs no
    /// barrier, which is what [`BufferState::untouched`] encodes.
    pub fn import_buffer(&mut self, name: &'static str, buffer: vk::Buffer) -> BufferId {
        let id = BufferId(u32::try_from(self.buffers.len()).unwrap_or(u32::MAX));
        self.buffers.push(RegisteredBuffer {
            buffer,
            state: BufferState::untouched(),
            name,
        });
        id
    }

    /// Add a pass that touches buffers as well as images.
    ///
    /// Both lists must be complete. An omitted buffer is the same defect an
    /// omitted image is, with one difference that makes it nastier: there is no
    /// layout to be wrong, so the picture is usually right and occasionally
    /// one frame stale.
    pub fn pass_with(
        &mut self,
        name: &'static str,
        accesses: &[(ImageId, Access)],
        buffers: &[(BufferId, BufferAccess)],
        record: impl FnOnce(&ash::Device, vk::CommandBuffer) + 'a,
    ) {
        self.passes.push(Pass {
            name,
            accesses: accesses.to_vec(),
            buffers: buffers.to_vec(),
            record: Box::new(record),
        });
    }

    /// Decide what a single buffer access needs, and advance its state.
    ///
    /// The buffer twin of [`Self::decide`], and split for the same reason:
    /// `plan` and `execute` must not contain two copies of this judgement.
    fn decide_buffer(
        &mut self,
        pass: &'static str,
        id: BufferId,
        access: BufferAccess,
    ) -> Option<BufferDecision> {
        let resource = self.buffers.get_mut(id.0 as usize)?;
        let target = BufferState {
            stage: access.stage(),
            access: access.access(),
        };

        // Nothing in *this* command buffer has touched it yet, so there is no
        // dependency to express. Cross-submission ordering is the fence's job,
        // and emitting a barrier against `TOP_OF_PIPE` with no source access
        // would be a barrier that says nothing.
        if resource.state.access.is_empty() {
            resource.state = target;
            return None;
        }

        // A hazard exists when either side writes. Read-after-read on a buffer
        // is already ordered, exactly as it is for an image in one layout.
        let wrote_before = resource.state.access.intersects(
            vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::TRANSFER_WRITE,
        );
        if !access.writes() && !wrote_before {
            // Still record the reader, so a later write knows to wait for it.
            resource.state.stage |= target.stage;
            resource.state.access |= target.access;
            return None;
        }

        let barrier = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(resource.state.stage)
            .src_access_mask(resource.state.access)
            .dst_stage_mask(target.stage)
            .dst_access_mask(target.access)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(resource.buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE);

        let transition = BufferTransition {
            pass,
            buffer: resource.name,
        };
        resource.state = target;
        Some((transition, barrier))
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
    pub fn plan(self) -> Vec<Transition> {
        self.plan_full().0
    }

    /// Image transitions and buffer barriers, without touching a device.
    #[must_use]
    pub fn plan_full(mut self) -> (Vec<Transition>, Vec<BufferTransition>) {
        let mut emitted = Vec::new();
        let mut buffers = Vec::new();
        let passes = std::mem::take(&mut self.passes);
        for pass in &passes {
            for (id, access) in &pass.accesses {
                if let Some((transition, _)) = self.decide(pass.name, *id, *access) {
                    emitted.push(transition);
                }
            }
            for (id, access) in &pass.buffers {
                if let Some((transition, _)) = self.decide_buffer(pass.name, *id, *access) {
                    buffers.push(transition);
                }
            }
        }
        (emitted, buffers)
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
            // which is the reading that does not smear neighbouring passes
            // into each other.
            //
            // **It does not cost overlap, and an earlier version of this
            // comment claimed it did.** A timestamp write is not an execution
            // dependency: it imposes no ordering the pipeline would not
            // otherwise have. Passes here happen to be ordered anyway, by the
            // graph's own barriers. Where two passes genuinely overlap, both
            // timestamps resolve against the same timeline and their intervals
            // will overlap too — which is a real limitation of reading these
            // numbers as a sum, but it is not a slowdown.
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
            let mut buffer_barriers = Vec::new();
            for (id, access) in &pass.buffers {
                if let Some((_, barrier)) = self.decide_buffer(pass.name, *id, *access) {
                    buffer_barriers.push(barrier);
                }
            }

            if !barriers.is_empty() || !buffer_barriers.is_empty() {
                let dependency = vk::DependencyInfo::default()
                    .image_memory_barriers(&barriers)
                    .buffer_memory_barriers(&buffer_barriers);
                // SAFETY: both slices outlive the call and every resource is
                // live for the caller-provided command buffer's lifetime.
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

/// The buffer twin of [`Decision`].
type BufferDecision = (BufferTransition, vk::BufferMemoryBarrier2<'static>);

/// A buffer barrier the graph decided to emit.
///
/// No layout to report, so this is only "who waited on whom, and where" — which
/// is still the thing a test needs to see, because the failure it guards
/// against is a barrier that was never emitted at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferTransition {
    pub pass: &'static str,
    pub buffer: &'static str,
}

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

    fn plan_buffers(build: impl FnOnce(&mut RenderGraph<'_>)) -> Vec<BufferTransition> {
        let mut graph = RenderGraph::new();
        build(&mut graph);
        graph.plan_full().1
    }

    /// **The rain frame's shape**: simulate into a buffer, then draw from it.
    /// Without a barrier between the two the draw reads last frame's drops,
    /// which looks almost right and is the reason buffers had to join the
    /// graph at all.
    #[test]
    fn a_compute_write_then_a_vertex_read_emits_one_buffer_barrier() {
        let barriers = plan_buffers(|g| {
            let drops = g.import_buffer("drops", vk::Buffer::null());
            g.pass_with("rain_simulate", &[], &[(drops, BufferAccess::ComputeReadWrite)], |_, _| {});
            g.pass_with("rain", &[], &[(drops, BufferAccess::VertexRead)], |_, _| {});
        });

        assert_eq!(barriers, [BufferTransition { pass: "rain", buffer: "drops" }]);
    }

    /// The first touch in a command buffer waits on nothing: whatever ran last
    /// frame was in a submission this one already fenced against.
    #[test]
    fn the_first_access_to_a_buffer_needs_no_barrier() {
        let barriers = plan_buffers(|g| {
            let drops = g.import_buffer("drops", vk::Buffer::null());
            g.pass_with("rain_simulate", &[], &[(drops, BufferAccess::ComputeReadWrite)], |_, _| {});
        });

        assert!(barriers.is_empty(), "{barriers:?}");
    }

    /// Two compute passes writing the same buffer in a row must not race —
    /// the write-after-write case, which has no layout change to give it away.
    #[test]
    fn compute_after_compute_on_one_buffer_still_emits_a_barrier() {
        let barriers = plan_buffers(|g| {
            let drops = g.import_buffer("drops", vk::Buffer::null());
            for _ in 0..3 {
                g.pass_with("tick", &[], &[(drops, BufferAccess::ComputeReadWrite)], |_, _| {});
            }
        });

        assert_eq!(barriers.len(), 2, "one between each consecutive pair");
    }

    /// Two readers in a row need nothing between them, and the writer after
    /// them gets exactly one barrier — which must wait on *both*, hence a read
    /// accumulating into the state rather than replacing it.
    #[test]
    fn a_write_after_two_reads_gets_one_barrier_that_waits_for_both() {
        let mut graph = RenderGraph::new();
        let drops = graph.import_buffer("drops", vk::Buffer::null());
        graph.pass_with("seed", &[], &[(drops, BufferAccess::TransferDst)], |_, _| {});
        graph.pass_with("draw", &[], &[(drops, BufferAccess::VertexRead)], |_, _| {});
        graph.pass_with("indirect", &[], &[(drops, BufferAccess::IndirectRead)], |_, _| {});
        graph.pass_with("simulate", &[], &[(drops, BufferAccess::ComputeReadWrite)], |_, _| {});

        // The barrier's source mask is built from the accumulated state, so
        // reach into it after planning rather than restating the rule here.
        let (_, barriers) = graph.plan_full();
        let passes: Vec<&str> = barriers.iter().map(|b| b.pass).collect();
        assert_eq!(passes, ["draw", "simulate"], "{barriers:?}");
    }

    /// And the accumulation itself, which is what makes the barrier above
    /// correct rather than merely present.
    #[test]
    fn a_read_accumulates_into_the_state_instead_of_replacing_it() {
        let mut graph = RenderGraph::new();
        let drops = graph.import_buffer("drops", vk::Buffer::null());
        graph.decide_buffer("seed", drops, BufferAccess::TransferDst);
        graph.decide_buffer("draw", drops, BufferAccess::VertexRead);
        graph.decide_buffer("indirect", drops, BufferAccess::IndirectRead);

        let state = graph.buffers[0].state;
        assert!(state.stage.contains(vk::PipelineStageFlags2::VERTEX_SHADER));
        assert!(state.stage.contains(vk::PipelineStageFlags2::DRAW_INDIRECT));
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
