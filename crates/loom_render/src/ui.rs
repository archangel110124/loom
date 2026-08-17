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

/// Pre-warp an authored screen-space sRGB hex into the byte egui must be given.
///
/// **This lives here, beside `srgb_framebuffer`, and not in the editor**, because
/// `ui.rs` is linked by the runtime as well: the HUD is game content that ships
/// to players. The encode fix below and this compensation are two halves of one
/// correction, and separating them would leave a shipped game's HUD holding only
/// the half that darkens it.
///
/// Three stages act on an egui colour and they do not cancel:
///
/// 1. `egui-ash-renderer-0.12.0/src/shaders/shader.vert:25` raises the vertex
///    colour to the power 2.2.
/// 2. `shader.frag:23` passes it through unchanged, because specialization
///    constant 0 (`SRGB_FRAMEBUFFER`) is `true` — see [`Ui::new`].
/// 3. The swapchain is `B8G8R8A8_SRGB`, so the hardware applies the sRGB encode.
///
/// So a hex `h` authored naively reaches the screen as `srgb_encode((h/255)^2.2)`,
/// about 36% darker: `#16191E` arrives as `#0E1218`. This is the exact inverse,
/// so what a table says is what the display shows.
///
/// **Correct only while `srgb_framebuffer` is `true`.** Under the `false` it was
/// set to before this was fixed, stage 2's `LINEARtoSRGB` cancels stage 1 exactly
/// and stage 3 is left uncompensated — `#16191E` arrives as `#535860`, *brighter*
/// rather than darker, and this function would then double the error instead of
/// removing it. The two settings need opposite corrections.
///
/// **No gate can see any of this.** `cargo xtask image` drives the offscreen
/// `Renderer`, which never constructs a [`Ui`]. The check is a human sampling
/// swatches off a screenshot.
#[must_use]
pub fn tok(hex: u32) -> egui::Color32 {
    let channel = |byte: u32| {
        #[allow(clippy::cast_precision_loss)]
        let s = (byte & 0xFF) as f32 / 255.0;
        // The sRGB *decode*, undoing stage 3.
        let linear = if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        };
        // The inverse of stage 1's `pow(2.2)`.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            (linear.powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8
        }
    };
    egui::Color32::from_rgb(channel(hex >> 16), channel(hex >> 8), channel(hex))
}

/// One frame of laid-out UI, between [`Ui::layout`] and [`Ui::record`].
///
/// Opaque on purpose: the split exists so that a rectangle read from egui is
/// current, not so that callers can inspect egui's tessellation.
pub struct UiFrame {
    primitives: Vec<egui::ClippedPrimitive>,
    textures_delta: egui::TexturesDelta,
    pixels_per_point: f32,
    /// What the panels left for the scene, in egui's logical points.
    scene: egui::Rect,
}

impl UiFrame {
    /// The rectangle the panels left for the scene, in **window pixels**.
    ///
    /// Current rather than a frame stale, which is the whole point of the
    /// split — it belongs to the frame that laid it out, so there is no way to
    /// read it from the wrong one.
    ///
    /// Converted from the logical points egui works in by this frame's
    /// `pixels_per_point`, because a swapchain is sized in pixels and
    /// conflating the two puts a HiDPI viewport half a panel off.
    #[must_use]
    pub fn scene_rect(&self) -> (i32, i32, u32, u32) {
        let scale = self.pixels_per_point;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            (
                (self.scene.min.x * scale).round() as i32,
                (self.scene.min.y * scale).round() as i32,
                (self.scene.width() * scale).round().max(0.0) as u32,
                (self.scene.height() * scale).round().max(0.0) as u32,
            )
        }
    }
}

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
                // **No depth attachment, because the UI has its own pass now.**
                // It used to draw inside the scene's rendering pass and had to
                // declare that pass's depth format or every UI draw was
                // VUID-...-08914. Rain moved it out: rain is a pass of its own
                // (the depth buffer changes from attachment to texture between
                // the two) and the UI has to come *after* it, or streaks fall
                // across the panels. It never tested or wrote depth anyway.
                depth_attachment_format: None,
                stencil_attachment_format: None,
            }),
            Options {
                in_flight_frames: 1,
                // The UI draws over a scene that already wrote depth. Testing
                // against it would clip panels behind geometry.
                enable_depth_test: false,
                enable_depth_write: false,
                // **The swapchain is `B8G8R8A8_SRGB` (`viewer.rs:2172`), so this
                // has to say so.** The crate's own words are "indicate whether
                // you will target an sRGB framebuffer"; it feeds the answer to
                // the fragment shader as specialization constant 0. Declaring
                // `false` against an `_SRGB` target left the hardware's encode
                // as a second, uncompensated conversion, and every UI colour
                // displayed lifted — a `#16191E` panel arriving as `#535860`,
                // and a contrast ratio designed at 14.6:1 landing near 6.7:1.
                //
                // **No golden image can see this.** `xtask image` drives the
                // offscreen `Renderer`, which never constructs a `Ui`, so the
                // gate cannot confirm or deny it — which is exactly why it went
                // unnoticed, and why the check is a human looking at the editor.
                //
                // It matters now rather than later because the editor rework
                // designs a palette from measured hex values (ADR 0033). Tuning
                // colours on top of this would produce a palette that only
                // looks right while the bug is present.
                srgb_framebuffer: true,
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

    /// The egui context, so a caller can install a theme on it.
    ///
    /// Read-only: `Context` is an `Arc` internally and its mutators take
    /// `&self`, so this hands out no more authority than it looks like.
    #[must_use]
    pub fn context(&self) -> &egui::Context {
        &self.context
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

    /// Run egui's layout for one frame and hand back what it produced.
    ///
    /// **This must happen before the render graph is built, not inside it.**
    /// The UI used to lay out and record in one call, from inside the `ui`
    /// pass closure — which `RenderGraph::execute` runs *after* the forward and
    /// tonemap closures have already recorded. So egui's layout for frame N
    /// happened after the scene for frame N was recorded, and any rectangle
    /// read from egui to place the scene could only ever be frame N−1's.
    ///
    /// That is invisible while the scene fills the window and glaring the
    /// moment it does not: dragging a splitter would show a stale scene rect
    /// against a live panel edge on every frame, and `chrome_clear` would paint
    /// the gap a tidy black that reads as intentional rather than as a bug.
    /// Splitting the call is the fix, and it has to land before anything reads
    /// a rectangle from the dock.
    ///
    /// The returned [`UiFrame`] must be given to [`Self::record`] in the same
    /// frame; holding one across frames leaks the texture delta.
    pub fn layout(
        &mut self,
        window: &winit::window::Window,
        mut build: impl FnMut(&mut egui::Ui),
    ) -> UiFrame {
        // egui 0.35 hands the closure a root `Ui` rather than a `Context`, and
        // panels attach to that Ui rather than to the context directly.
        let input = self.winit.take_egui_input(window);
        // **The scene rect is read here, immediately after the panels are
        // added and while the root `Ui` still exists.** `egui::Context` has no
        // `available_rect` in 0.35 — the answer lives on the root `Ui`, which
        // `run_ui` owns and drops — so the closure is wrapped rather than the
        // context queried afterwards. This is the same
        // `available_rect_before_wrap` the HUD anchors to.
        let mut scene = egui::Rect::ZERO;
        let output = self.context.run_ui(input, |ui| {
            build(ui);
            scene = ui.available_rect_before_wrap();
        });
        self.winit
            .handle_platform_output(window, output.platform_output);
        let primitives = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        UiFrame {
            primitives,
            textures_delta: output.textures_delta,
            pixels_per_point: output.pixels_per_point,
            scene,
        }
    }

    /// Record a laid-out frame into the command buffer.
    ///
    /// Stays inside the `ui` render-graph pass, because that is where the
    /// target is in `COLOR_ATTACHMENT_OPTIMAL` and the rendering block is open.
    /// Nothing here computes a layout, so nothing here can be a frame stale.
    ///
    /// # Errors
    /// [`RenderError`] if egui's renderer fails to upload or record.
    pub fn record(
        &mut self,
        frame: &UiFrame,
        cmd: vk::CommandBuffer,
        extent: vk::Extent2D,
        queue: vk::Queue,
        pool: vk::CommandPool,
    ) -> Result<(), RenderError> {
        if !frame.textures_delta.set.is_empty() {
            // Font atlas and any user textures. Uploaded on the graphics queue
            // and pool the caller passes in — the only queue this engine uses,
            // since Vulkan doc §6's multi-queue work is deferred.
            self.renderer
                .set_textures(queue, pool, frame.textures_delta.set.as_slice())
                .map_err(|e| RenderError::Allocator(e.to_string()))?;
        }
        self.renderer
            .cmd_draw(cmd, extent, frame.pixels_per_point, &frame.primitives)
            .map_err(|e| RenderError::Allocator(e.to_string()))?;
        if !frame.textures_delta.free.is_empty() {
            self.renderer.free_textures(&frame.textures_delta.free).ok();
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

    /// Whether a **text field** has focus, as opposed to any widget at all.
    ///
    /// The distinction matters for keys egui claims structurally. Tab is its
    /// focus key, so `wants_keyboard` is true the moment focus lands anywhere
    /// — including a toolbar button — and using it to decide whether the
    /// viewport may see Tab latched the key permanently held.
    #[must_use]
    pub fn wants_text_input(&self) -> bool {
        self.context.text_edit_focused()
    }
}
