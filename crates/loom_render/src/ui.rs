//! The editor's UI layer: egui, drawn over the scene in the same pass.
//!
//! Design doc §2.1 is why M5.5 called the viewer cheap: `loom_reflect` already
//! produces a JSON Schema per component, carrying field names, types, ranges,
//! and doc comments. **The inspector is generated from that**, not
//! hand-written per type — which is the same property that keeps the agent's
//! tool API from drifting.
//!
//! Drawn with dynamic rendering into the swapchain image the scene just wrote,
//! so there is no second pass and no render-pass object (never-do #1).

use ash::vk;
use egui_ash_renderer::allocator::GpuAllocator;
use egui_ash_renderer::{DynamicRendering, Options, Renderer as EguiRenderer};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};

use crate::renderer::RenderError;
use crate::{Device, Instance};

/// egui, wired to Vulkan and winit.
pub struct Ui {
    context: egui::Context,
    winit: egui_winit::State,
    renderer: EguiRenderer<GpuAllocator>,
    /// Kept alive: the renderer holds buffers from it.
    _allocator: std::sync::Arc<std::sync::Mutex<Allocator>>,
}

impl Ui {
    /// Build the UI layer for a window and swapchain format.
    ///
    /// # Errors
    /// [`RenderError`] if the allocator or egui renderer cannot be created.
    pub fn new(
        instance: &Instance,
        device: &Device,
        window: &winit::window::Window,
        color_format: vk::Format,
    ) -> Result<Self, RenderError> {
        // egui gets its own allocator rather than sharing the viewer's behind
        // an Arc<Mutex<>>. Both are suballocators over `vkAllocateMemory`, so
        // a second one costs a handful of blocks — considerably less than
        // threading interior mutability through every resource the renderer
        // owns for the sake of one library's constructor signature.
        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.handle().clone(),
            device: device.handle().clone(),
            physical_device: device.physical(),
            debug_settings: gpu_allocator::AllocatorDebugSettings::default(),
            buffer_device_address: true,
            allocation_sizes: gpu_allocator::AllocationSizes::default(),
        })
        .map_err(|e| RenderError::Allocator(e.to_string()))?;
        let allocator = std::sync::Arc::new(std::sync::Mutex::new(allocator));

        let context = egui::Context::default();
        let winit = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            #[allow(clippy::cast_possible_truncation)]
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        let renderer = EguiRenderer::with_gpu_allocator(
            allocator.clone(),
            device.handle().clone(),
            egui_ash_renderer::RenderMode::DynamicRendering(DynamicRendering {
                color_attachment_format: color_format,
                // The UI draws inside the *scene's* rendering pass, which has a
                // depth attachment — so egui's pipeline has to declare the same
                // format or every UI draw is VUID-...-08914. It still neither
                // tests nor writes depth (see `Options` below); declaring the
                // format is about matching the pass, not about using it.
                depth_attachment_format: Some(crate::renderer::DEPTH_FORMAT),
                stencil_attachment_format: None,
            }),
            Options {
                in_flight_frames: 1,
                // The UI draws over a scene that already wrote depth. Testing
                // against it would clip panels behind geometry.
                enable_depth_test: false,
                enable_depth_write: false,
                srgb_framebuffer: false,
            },
        )
        .map_err(|e| RenderError::Allocator(e.to_string()))?;

        Ok(Self {
            context,
            winit,
            renderer,
            _allocator: allocator,
        })
    }

    /// Feed a window event to egui.
    ///
    /// Returns `true` when egui consumed it — the caller must then **not** act
    /// on it, or clicking a panel also moves the camera behind it.
    pub fn on_window_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> bool {
        self.winit.on_window_event(window, event).consumed
    }

    /// Build and record the UI for one frame.
    ///
    /// # Errors
    /// [`RenderError`] if egui's renderer fails to record.
    pub fn draw(
        &mut self,
        window: &winit::window::Window,
        cmd: vk::CommandBuffer,
        extent: vk::Extent2D,
        queue: vk::Queue,
        pool: vk::CommandPool,
        build: impl FnMut(&mut egui::Ui),
    ) -> Result<(), RenderError> {
        // egui 0.35 hands the closure a root `Ui` rather than a `Context`, and
        // panels attach to that Ui rather than to the context directly.
        let input = self.winit.take_egui_input(window);
        let output = self.context.run_ui(input, build);
        self.winit
            .handle_platform_output(window, output.platform_output);

        let primitives = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        if !output.textures_delta.set.is_empty() {
            // Font atlas and any user textures. Uploaded on the graphics queue
            // and pool the caller passes in — the only queue this engine uses,
            // since Vulkan doc §6's multi-queue work is deferred.
            self.renderer
                .set_textures(queue, pool, output.textures_delta.set.as_slice())
                .map_err(|e| RenderError::Allocator(e.to_string()))?;
        }
        self.renderer
            .cmd_draw(cmd, extent, output.pixels_per_point, &primitives)
            .map_err(|e| RenderError::Allocator(e.to_string()))?;
        if !output.textures_delta.free.is_empty() {
            self.renderer.free_textures(&output.textures_delta.free).ok();
        }
        Ok(())
    }

    /// Whether the pointer is over a panel, so the caller can ignore the click.
    #[must_use]
    pub fn wants_pointer(&self) -> bool {
        self.context.egui_wants_pointer_input()
    }

    #[must_use]
    pub fn wants_keyboard(&self) -> bool {
        self.context.egui_wants_keyboard_input()
    }
}
