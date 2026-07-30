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
}

impl<'a> RenderGraph<'a> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            images: Vec::new(),
            passes: Vec::new(),
        }
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

        for pass in passes {
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
