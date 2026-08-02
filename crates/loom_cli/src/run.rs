//! `loom run` — a window showing the scene, with a fly camera and the editor.
//!
//! The window is a **live view of the file**, not a snapshot of it. Every edit
//! — a human dragging a gizmo, or the agent applying a transaction through the
//! CLI while this window is open — re-derives the scene and shows up in the
//! viewport. That is what `--watch` in brief §2's M5.5 is for, and it is what
//! makes the human's oversight of the agent real rather than nominal.
//!
//! §7.17 governs the collision: the watcher reloads, and it never merges. When
//! the human has unsaved work and the file moves underneath, both versions are
//! kept and the human picks which one survives.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use loom_input::{ActionMap, InputState};
use loom_render::glam::Vec3;
use loom_render::{Camera, Device, Instance, Ui, Viewer, ash, ash_window};

use crate::gizmo::{self, Mode};
use crate::panels::{PanelState, UiAction};
use crate::scene_view::{Change, SceneView};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

/// Metres per second.
const MOVE_SPEED: f32 = 6.0;
const SPRINT_MULTIPLIER: f32 = 3.0;
/// Radians per pixel of mouse motion.
const LOOK_SENSITIVITY: f32 = 0.0025;
/// Degrees per world-unit of drag, in rotate mode.
const ROTATE_PER_UNIT: f32 = 45.0;

/// How often the scene file is checked for someone else's writes.
///
/// A human reads a quarter second as "immediate" and it costs one small read of
/// a text file. Fast enough to feel live, cheap enough to be free.
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// An orbit-free fly camera: position plus yaw/pitch.
struct FlyCamera {
    position: Vec3,
    yaw: f32,
    pitch: f32,
    /// Vertical FOV in degrees, taken from an authored `Camera` when the scene
    /// has one. A first-person scene authored at 90 has to *look* like 90 here,
    /// or the window is not showing what the scene says.
    fov_y_degrees: f32,
}

/// What the window opens on when a scene has no camera of its own.
const DEFAULT_FOV: f32 = 60.0;

impl FlyCamera {
    /// Frame some bounds, then look at their centre — so the window opens on
    /// the content rather than on empty space.
    fn framing(bounds: (Vec3, f32)) -> Self {
        Self::framing_at(bounds, DEFAULT_FOV)
    }

    /// Frame bounds while keeping a lens. Focusing on a node moves the camera;
    /// it is not a reason to change the field of view the scene asked for.
    fn framing_at(bounds: (Vec3, f32), fov_y_degrees: f32) -> Self {
        let (center, radius) = bounds;
        let distance = (radius * 2.2).max(4.0);
        let position = center + Vec3::new(0.6, 0.45, 1.0).normalize() * distance;

        Self::looking(position, center - position, fov_y_degrees)
    }

    /// Start where the scene's `Camera` sits, looking where it looks.
    ///
    /// The fly camera is still free afterwards — this sets where the window
    /// *opens*, it does not hand the view over. A human who wants to inspect
    /// the level from behind the player must still be able to fly there.
    fn at(view: loom_ecs::CameraView) -> Self {
        let position = Vec3::from_array(view.eye);
        Self::looking(
            position,
            Vec3::from_array(view.target) - position,
            view.fov_y_degrees,
        )
    }

    fn looking(position: Vec3, direction: Vec3, fov_y_degrees: f32) -> Self {
        let direction = direction.normalize_or_zero();
        Self {
            position,
            yaw: direction.x.atan2(direction.z),
            pitch: direction.y.asin(),
            fov_y_degrees,
        }
    }

    fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        )
    }

    fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize_or_zero()
    }

    fn camera(&self) -> Camera {
        Camera {
            eye: self.position,
            target: self.position + self.forward(),
            fov_y_degrees: self.fov_y_degrees,
        }
    }
}

/// The context the viewer runs in. A menu would push another.
const FLY: &str = "fly";
/// Driving a character while Play runs. Live only then.
const PLAY: &str = "play";
/// Editing actions, live only when `--edit` was passed.
const EDIT: &str = "edit";
/// How far one nudge moves a node, in metres.
const NUDGE: f32 = 0.25;

/// A gizmo handle being dragged.
struct Drag {
    /// The handle as it was when grabbed. Frozen, so the gearing does not
    /// change under the cursor as the node moves away from where it started.
    handle: gizmo::Handle,
    /// Where the press landed, in window pixels.
    from: (f32, f32),
    /// The node's transform when the drag started. Every frame sets an
    /// absolute value derived from this rather than accumulating deltas, so a
    /// dropped frame cannot make the node drift.
    start: [[f32; 3]; 3],
    node: String,
}

struct App {
    /// Everything derived from the scene text. Rebuilt on every change, which
    /// is what makes the window follow the file.
    view: SceneView,
    /// Directory the scene lives in, for resolving its assets on rebuild.
    base: std::path::PathBuf,
    /// Reused across rebuilds so a drag does not re-bake voxel volumes.
    voxels: crate::VoxelCache,
    /// `Some` when `--edit` was passed. Read-only otherwise.
    session: Option<loom_scene::Session>,
    /// Selected node paths. Paths rather than indices: the agent can insert a
    /// node above yours between frames, and an index would then be pointing at
    /// somebody else's object.
    selected: Vec<String>,
    /// Where the scene lives, for watching and for reloading.
    scene_path: std::path::PathBuf,
    /// egui, when editing. `None` in read-only mode, so a viewer costs no UI.
    ui: Option<Ui>,
    /// Cursor position in window pixels, for picking and for gizmo drags.
    cursor: (f32, f32),
    registry: loom_reflect::TypeRegistry,
    dirty: bool,
    /// The disk's version of the scene, held back because taking it would
    /// discard unsaved work. §7.17: never merge — let the human choose.
    conflict: Option<String>,
    /// Version of the file as we last read or wrote it. Anything else on disk
    /// is somebody else's write.
    disk_seen: loom_scene::VersionToken,
    /// Mesh set currently on the GPU, so a moved node costs no re-upload.
    uploaded: u64,
    /// The handle being dragged, if one is.
    drag: Option<Drag>,
    /// Bumped whenever a mouse button comes up. Part of every gesture key, so
    /// letting go and dragging the same handle again is a second undo step
    /// rather than a continuation of the first.
    gesture_epoch: u32,
    /// The running simulation, when Play is on. `None` is edit mode.
    ///
    /// Play mode holds its own world and never writes the file, so Stop cannot
    /// lose work — the authored scene was never touched.
    play: Option<crate::play::Play>,
    /// What the gizmo edits: Unity's W/E/R, on digits here.
    mode: Mode,
    /// Draw calls for the simulated world, refreshed only on ticks that
    /// actually ran.
    play_objects: Vec<loom_render::Object>,
    /// How many script-fired explosions have already been given to the
    /// particle systems. The runner's list only grows, so the tail past this
    /// is what is new — no event queue, and nothing to miss if a frame is slow.
    detonations_seen: usize,
    /// Whether the pointer is captured for first-person play. Tracked rather
    /// than asked of the window because winit has no getter, and because
    /// releasing it must not depend on the platform honouring the request.
    captured: bool,
    /// Live particle state, kept across frames. `None` until the first frame
    /// after a scene load.
    plumes: Option<crate::particles::Plumes>,
    /// Handles as of the last frame, so a mouse press can hit-test them
    /// against exactly what was drawn.
    handles: Vec<gizmo::Handle>,
    /// When to next look at the file.
    next_watch: std::time::Instant,
    camera: FlyCamera,
    /// Loaded from TOML, so rebinding needs no rebuild.
    bindings: ActionMap,
    input: InputState,
    window: Option<Arc<Window>>,
    viewer: Option<Viewer>,
    /// Kept alive for the whole session: destroying the device before the
    /// viewer's resources would be a use-after-free.
    gpu: Option<(Instance, Device)>,
    last_frame: std::time::Instant,
    /// Smoothed, because a number that changes sixty times a second is not a
    /// number anyone can read.
    fps: f32,
    /// What somebody else changed, and when we noticed.
    ///
    /// The whole premise of this editor is that an agent is authoring the file
    /// while a human watches. "The scene changed on disk" is true and useless;
    /// this is which nodes, so the viewport can point at them.
    agent_changes: Vec<(Change, std::time::Instant)>,
    /// Frames still to draw before shutting down, when `--frames` was passed.
    ///
    /// Exists so the **whole lifecycle** — create, draw, tear down — can be
    /// run unattended under the validation layers. Every teardown bug found so
    /// far needed a human to open a window and close it; this is what lets
    /// `cargo xtask validate` do that instead.
    frames_left: Option<u32>,
    title: String,
}

/// Tear down in an order the driver can survive.
///
/// Struct fields drop in **declaration order**, and this struct's had `window`
/// before `viewer` — so the X11 window was destroyed first and then
/// `Viewer::drop` destroyed a `VkSurfaceKHR` that still referenced it. The
/// `SAFETY` note on `create_surface` asserted the opposite of what was
/// happening, which is how it survived review.
///
/// Written out rather than fixed by reordering fields, because a reorder is
/// invisible and someone alphabetising this struct in a year would put the
/// use-after-free straight back.
impl Drop for App {
    fn drop(&mut self) {
        // **The viewer before the UI.** egui records its draws into the
        // viewer's command buffer, so egui's pipeline and descriptor pool are
        // still referenced by it until the viewer destroys its command pool.
        // Dropping the UI first is VUID-vkDestroyPipeline-pipeline-00765 and
        // VUID-vkDestroyDescriptorPool-descriptorPool-00303.
        //
        // The surface's window must still exist here, which is why the window
        // is released further down rather than by field order.
        self.viewer = None;
        self.ui = None;

        // **A device is a child of its instance and must die first.** This was
        // held as `Option<(Instance, Device)>`, and a tuple drops `.0` before
        // `.1` — so `vkDestroyInstance` ran while the `VkDevice` was still
        // alive, and `vkDestroyDevice` then ran against an instance that no
        // longer existed. On this box that segfaults inside the NVIDIA driver
        // at process teardown and briefly wedges the display.
        //
        // The headless path got this right by accident: it holds them as two
        // locals, and locals drop in reverse declaration order.
        if let Some((instance, device)) = self.gpu.take() {
            drop(device);
            drop(instance);
        }

        // The window last. Nothing Vulkan-side refers to it any more.
        self.window = None;
    }
}

impl App {
    fn new(
        view: SceneView,
        title: String,
        session: Option<loom_scene::Session>,
        scene_path: std::path::PathBuf,
        disk_seen: loom_scene::VersionToken,
    ) -> Self {
        let base = scene_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        Self {
            // The scene's own camera when it has one, the whole scene framed
            // when it does not.
            camera: view
                .camera()
                .map_or_else(|| FlyCamera::framing(view.bounds), FlyCamera::at),
            uploaded: view.mesh_key,
            selected: view.paths.first().cloned().into_iter().collect(),
            view,
            base,
            voxels: crate::VoxelCache::default(),
            session,
            scene_path,
            disk_seen,
            drag: None,
            gesture_epoch: 0,
            play: None,
            mode: Mode::Move,
            handles: Vec::new(),
            play_objects: Vec::new(),
            detonations_seen: 0,
            captured: false,
            plumes: None,
            ui: None,
            cursor: (0.0, 0.0),
            registry: loom_scene::components::registry(),
            dirty: false,
            conflict: None,
            // `clippy.toml` disallows `Instant::now`, and it is right to fire.
            // The rule is that SIMULATION must not read the wall clock
            // (never-do #8, §7.5) — a deterministic tick cannot depend on how
            // fast the machine is. This is the presentation loop: frame pacing
            // and file polling for a human at a window, which is exactly what
            // wall time is for. Scoped to these reads rather than weakening the
            // lint for the crate.
            #[allow(clippy::disallowed_methods)]
            next_watch: std::time::Instant::now(),
            // Prefer a project-local file, fall back to the shipped defaults,
            // so a fresh checkout has a working camera with no config to write.
            bindings: load_bindings(),
            input: InputState::new(),
            window: None,
            viewer: None,
            gpu: None,
            #[allow(clippy::disallowed_methods)]
            last_frame: std::time::Instant::now(),
            fps: 0.0,
            frames_left: None,
            agent_changes: Vec::new(),
            title,
        }
    }

    /// How long an agent's change stays outlined in the viewport.
    ///
    /// Long enough to look up from what you were doing, short enough that a
    /// busy agent does not leave the screen permanently boxed.
    const CHANGE_FADE: f32 = 6.0;

    /// Screen-space boxes for whatever somebody else changed recently.
    fn agent_marks(
        &mut self,
        projection: &gizmo::View,
        now: std::time::Instant,
    ) -> Vec<crate::panels::AgentMark> {
        self.agent_changes
            .retain(|(_, at)| now.duration_since(*at).as_secs_f32() < Self::CHANGE_FADE);

        let mut marks = Vec::new();
        for (change, at) in &self.agent_changes {
            // A removed node has no bounds left to point at; the console line
            // is the only honest thing to show for it.
            let Some(bounds) = self.view.node_bounds(&change.path) else {
                continue;
            };
            let (mut x0, mut y0) = (f32::MAX, f32::MAX);
            let (mut x1, mut y1) = (f32::MIN, f32::MIN);
            let mut visible = false;
            for corner in 0..8 {
                let point = Vec3::new(
                    if corner & 1 == 0 { bounds.min[0] } else { bounds.max[0] },
                    if corner & 2 == 0 { bounds.min[1] } else { bounds.max[1] },
                    if corner & 4 == 0 { bounds.min[2] } else { bounds.max[2] },
                );
                if let Some((sx, sy)) = projection.project(point) {
                    x0 = x0.min(sx);
                    y0 = y0.min(sy);
                    x1 = x1.max(sx);
                    y1 = y1.max(sy);
                    visible = true;
                }
            }
            if !visible {
                continue;
            }
            let age = now.duration_since(*at).as_secs_f32();
            let name = change.path.rsplit('/').next().unwrap_or(&change.path);
            marks.push(crate::panels::AgentMark {
                rect: (x0, y0, x1, y1),
                label: format!("{name} · {}", change.kind.label()),
                freshness: 1.0 - (age / Self::CHANGE_FADE),
            });
        }
        marks
    }

    /// The one selected node, when exactly one is.
    fn focused(&self) -> Option<String> {
        (self.selected.len() == 1)
            .then(|| self.selected.first().cloned())
            .flatten()
    }

    /// Re-derive, and remember what somebody else changed while doing it.
    fn show_external(&mut self, text: &str) {
        // Snapshot the nodes, not the whole view: rebuilding the previous
        // scene just to compare it would re-bake every voxel volume.
        let before: Vec<loom_scene::Node> = self.view.scene.nodes().to_vec();
        self.show(text);
        {
            let changes = self.view.changes_from(&before);
            if !changes.is_empty() {
                crate::log::info(format!(
                    "the agent changed {} node(s): {}",
                    changes.len(),
                    changes
                        .iter()
                        .take(4)
                        .map(|c| c.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            #[allow(clippy::disallowed_methods)]
            let now = std::time::Instant::now();
            self.agent_changes = changes.into_iter().map(|c| (c, now)).collect();
        }
    }

    /// Re-derive the view from scene text and hand any new geometry to the GPU.
    ///
    /// Invalid text — which is what a scene looks like halfway through somebody
    /// else's write — leaves the last good view on screen and says so. Blanking
    /// the viewport because a file was caught mid-save would be worse than
    /// useless.
    fn show(&mut self, text: &str) {
        let view = match SceneView::build_cached(text, &self.base, &mut self.voxels) {
            Ok(view) => view,
            Err(e) => {
                crate::log::error(format!("scene did not parse; showing the last good one: {e}"));
                return;
            }
        };

        if view.mesh_key != self.uploaded
            && let Some(viewer) = self.viewer.as_mut()
        {
            match viewer.set_meshes(view.meshes()) {
                Ok(()) => self.uploaded = view.mesh_key,
                Err(e) => crate::log::error(format!("could not upload the new geometry: {e}")),
            }
        }

        // A node the agent deleted cannot stay selected.
        self.selected.retain(|p| view.paths.contains(p));
        if self.selected.is_empty() {
            self.selected.extend(view.paths.first().cloned());
        }
        self.view = view;
        // The scene changed, so the emitters may have. Dropped rather than
        // patched: a reload is rare and rebuilding is a warm-up, not a frame
        // cost.
        self.plumes = None;
    }

    /// Re-derive from whatever the session currently holds.
    fn resync(&mut self) {
        let Some(text) = self.session.as_ref().map(|s| s.text().to_owned()) else {
            return;
        };
        self.show(&text);
    }

    /// Notice somebody else writing the scene.
    ///
    /// `ponytail:` polls and re-reads the file rather than taking an inotify
    /// dependency. A scene is kilobytes and this runs four times a second.
    /// Switch to `notify` if scenes reach megabytes, or if watching a whole
    /// asset tree starts to matter.
    fn poll_file(&mut self, now: std::time::Instant) {
        if now < self.next_watch {
            return;
        }
        self.next_watch = now + WATCH_INTERVAL;

        let Ok(disk) = std::fs::read_to_string(&self.scene_path) else {
            return;
        };
        // Against the version we last read or wrote — comparing against our own
        // in-memory text would flag every unsaved edit as somebody else's write.
        let version = loom_scene::VersionToken::of(&disk);
        if version == self.disk_seen {
            return;
        }

        if self.session.is_some() && self.dirty {
            // Two divergent versions. never-do #15 and §7.17: reject, and let
            // the human choose — do NOT merge, and do NOT drop either side.
            if self.conflict.is_none() {
                crate::log::warn("the scene changed on disk while you have unsaved edits");
            }
            self.conflict = Some(disk);
            return;
        }

        self.disk_seen = version;
        self.conflict = None;
        crate::log::info("the scene changed on disk — reloaded");
        match self.session.as_mut() {
            Some(session) => {
                if let Err(e) = session.reload() {
                    crate::log::error(format!("reload failed: {e}"));
                    return;
                }
                let text = self.session.as_ref().map(|s| s.text().to_owned());
                if let Some(text) = text {
                    self.show_external(&text);
                }
            }
            // Read-only. Nothing of ours to lose, so just follow the file.
            None => self.show_external(&disk),
        }
    }

    /// Take the disk's version, discarding unsaved edits. The human's call.
    fn accept_disk(&mut self) {
        let Some(disk) = self.conflict.take() else {
            return;
        };
        self.disk_seen = loom_scene::VersionToken::of(&disk);
        self.dirty = false;
        match self.session.as_mut() {
            Some(session) => {
                if let Err(e) = session.reload() {
                    crate::log::error(format!("reload failed: {e}"));
                    return;
                }
                self.resync();
            }
            None => self.show(&disk),
        }
        crate::log::info("reloaded from disk; unsaved edits discarded");
    }

    /// Keep the in-memory version. Saving will then overwrite the file, which
    /// is why this says so rather than doing it quietly.
    fn keep_mine(&mut self) {
        let Some(disk) = self.conflict.take() else {
            return;
        };
        self.disk_seen = loom_scene::VersionToken::of(&disk);
        // The session has to be told too, or the next Ctrl+S is refused and
        // the human is locked out of saving the version they just chose —
        // with the only escape being the button that discards it.
        if let Some(session) = self.session.as_mut()
            && let Err(e) = session.accept_disk_version()
        {
            crate::log::error(format!("could not take the disk version as a baseline: {e}"));
            return;
        }
        crate::log::warn("keeping your version — saving will overwrite what is on disk");
    }

    fn step_camera(&mut self, dt: f32) {
        let active = |a: &str| self.input.is_active(&self.bindings, FLY, a);
        let axis = |p: &str, n: &str| self.input.axis(&self.bindings, FLY, p, n);

        let speed = if active("sprint") {
            MOVE_SPEED * SPRINT_MULTIPLIER
        } else {
            MOVE_SPEED
        } * dt;

        // World up for vertical, not camera up: rising should be vertical even
        // when looking down, which is what every fly camera does.
        let delta = self.camera.forward() * axis("move_forward", "move_back")
            + self.camera.right() * axis("move_right", "move_left")
            + Vec3::Y * axis("move_up", "move_down");

        self.camera.position += delta.normalize_or_zero() * speed;
    }

    /// Whether the look-around action is held.
    fn looking(&self) -> bool {
        self.input.is_active(&self.bindings, FLY, "look")
    }

    /// Bounds of the selection, or of the whole scene when nothing is picked.
    fn focus_bounds(&self) -> (Vec3, f32) {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut any = false;
        for path in &self.selected {
            if let Some(b) = self.view.node_bounds(path) {
                min = min.min(Vec3::from_array(b.min));
                max = max.max(Vec3::from_array(b.max));
                any = true;
            }
        }
        if any {
            ((min + max) * 0.5, ((max - min).length() * 0.5).max(0.5))
        } else {
            self.view.bounds
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1440, 900));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("loom: could not create a window: {e}");
                event_loop.exit();
                return;
            }
        };

        match build_viewer(
            &window,
            self.view.meshes(),
            self.view.textures(),
            self.view.material_table(),
        ) {
            Ok((instance, device, viewer)) => {
                crate::log::info(format!("rendering on {}", device.name()));
                match Ui::new(&instance, &device, &window, viewer.color_format()) {
                    Ok(ui) => self.ui = Some(ui),
                    Err(e) => crate::log::error(format!("no editor UI ({e}); continuing bare")),
                }
                self.gpu = Some((instance, device));
                self.viewer = Some(viewer);
                self.window = Some(window);
            }
            Err(e) => {
                eprintln!("loom: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Once the close has been accepted the X window is already gone, but
        // winit still delivers the events queued behind it — including a
        // `RedrawRequested`. Drawing one asks the window for its size, X
        // answers `BadDrawable`, and winit unwraps that into a panic. So stop
        // touching the window the moment we are on the way out.
        if event_loop.exiting() {
            return;
        }

        // egui sees every event first. If it consumed one, the viewport must
        // NOT also act on it — otherwise clicking a panel also moves the camera
        // behind it and typing a number in the inspector flies the camera.
        let mut consumed = match (self.ui.as_mut(), self.window.as_ref()) {
            (Some(ui), Some(window)) => ui.on_window_event(window, &event),
            _ => false,
        };
        // egui reports Tab as consumed unconditionally — it is its focus key —
        // so `select_next` bound to Tab never reached the input map.
        //
        // `wants_keyboard` is the wrong question: it is true whenever *any*
        // widget has focus, including a toolbar button, so after the first Tab
        // moved focus it stayed true and the release event was swallowed. The
        // key then latched held forever and every subsequent frame re-fired
        // `select_next`. Both press and release have to arrive, or neither.
        let is_tab = matches!(
            event,
            WindowEvent::KeyboardInput {
                event: winit::event::KeyEvent {
                    physical_key: PhysicalKey::Code(winit::keyboard::KeyCode::Tab),
                    ..
                },
                ..
            }
        );
        if consumed && is_tab && !self.ui.as_ref().is_some_and(Ui::wants_text_input) {
            consumed = false;
        }
        if consumed && !matches!(event, WindowEvent::RedrawRequested | WindowEvent::Resized(_)) {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                crate::log::info("closing");
                self.shutdown(event_loop);
            }
            WindowEvent::Destroyed => self.shutdown(event_loop),

            // Keys are recorded by NAME and interpreted by the action map.
            // Nothing here knows what W means.
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.input
                        .set_button(&format!("{code:?}"), event.state == ElementState::Pressed);
                }
                // Nothing acts on the key here. `Pressed` is latched for the
                // whole frame, so evaluating it per event re-fires for every
                // further keyboard event in that frame — the same bug already
                // fixed for the editing actions. All of them are read once, in
                // the redraw.
            }

            WindowEvent::CursorMoved { position, .. } => {
                #[allow(clippy::cast_possible_truncation)]
                {
                    self.cursor = (position.x as f32, position.y as f32);
                }
                self.drag_gizmo();
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let over_panel = self.ui.as_ref().is_some_and(Ui::wants_pointer);
                if button == MouseButton::Left && !over_panel {
                    match state {
                        ElementState::Pressed => self.press_in_viewport(),
                        ElementState::Released => self.drag = None,
                    }
                }
                if state == ElementState::Released {
                    self.drag = None;
                    self.gesture_epoch = self.gesture_epoch.wrapping_add(1);
                }
                let name = match button {
                    MouseButton::Left => "MouseLeft",
                    MouseButton::Right => "MouseRight",
                    MouseButton::Middle => "MouseMiddle",
                    _ => return,
                };
                self.input.set_button(name, state == ElementState::Pressed);
            }

            WindowEvent::Resized(size) => {
                // The new size is passed along rather than discarded: a
                // compositor may decline to state one, and then this is the
                // only party that knows.
                if let Some(viewer) = self.viewer.as_mut()
                    && let Err(e) = viewer.recreate_sized(size.width, size.height)
                {
                    eprintln!("loom: resize failed: {e}");
                    event_loop.exit();
                }
            }

            WindowEvent::RedrawRequested => {
                // See the note in `App::new` — presentation, not simulation.
                #[allow(clippy::disallowed_methods)]
                let now = std::time::Instant::now();
                let dt = now.duration_since(self.last_frame).as_secs_f32();
                self.last_frame = now;
                if dt > 0.0 {
                    // Exponential smoothing: readable, and one line.
                    self.fps = self.fps.mul_add(0.9, (1.0 / dt) * 0.1);
                }
                // Clamp: a stall must not teleport the camera across the map.
                self.step_camera(dt.min(0.1));
                // Once per frame, not once per keyboard event: `end_frame`
                // clears the pressed-this-frame transitions per redraw, so
                // running this per event fired a single keypress once for
                // every further event in the same frame — one tap of Delete
                // could remove several nodes.
                //
                // `InputState` latches a press until the frame ends (see
                // `pressed_this_frame`), so a key tapped and released between
                // two redraws is still seen exactly once here rather than
                // missed entirely.
                if self.input.is_active(&self.bindings, FLY, "quit") {
                    // While driving a character, Escape means "give me my
                    // cursor back", not "close the window" — which is what it
                    // means in every game that ever captured a pointer.
                    if self.captured {
                        self.capture_pointer(false);
                    } else {
                        event_loop.exit();
                        return;
                    }
                }
                if self.input.is_active(&self.bindings, FLY, "reframe") {
                    // Unity's F: frame the selection, or the scene when there
                    // is none. Framing the whole level when you meant one desk
                    // is the more annoying of the two mistakes.
                    self.camera = FlyCamera::framing_at(self.focus_bounds(), self.camera.fov_y_degrees);
                }
                self.handle_editing();
                // Watching the file while a simulation runs would reload the
                // authored scene out from under it; the sim owns the world
                // until Stop.
                if self.play.is_none() {
                    self.poll_file(now);
                }
                self.feed_play_input();
                // How many simulation ticks this frame actually ran. Particles
                // advance by exactly this, so they keep time with the physics
                // instead of with the frame rate.
                let before = self.play.as_ref().map_or(0, |p| p.ticks);
                if self.play.as_mut().is_some_and(|p| p.advance(dt)) {
                    self.refresh_play_objects();
                    self.spawn_new_detonations();
                }
                let stepped = self.play.as_ref().map_or(0, |p| p.ticks.saturating_sub(before));

                // While driving a character the view is the scene's camera,
                // which the character carries. The fly camera is still there
                // and still where it was — Escape hands the pointer back and
                // Stop returns to it.
                let camera = self
                    .play
                    .as_ref()
                    .filter(|_| self.captured)
                    .and_then(crate::play::Play::camera)
                    .map_or_else(
                        || self.camera.camera(),
                        |view| Camera {
                            eye: Vec3::from_array(view.eye),
                            target: Vec3::from_array(view.target),
                            fov_y_degrees: view.fov_y_degrees,
                        },
                    );
                let extent = self.viewer.as_ref().map_or((1, 1), Viewer::extent);
                #[allow(clippy::cast_precision_loss)]
                let projection = gizmo::View::new(&camera, extent.0 as f32, extent.1 as f32);

                // Handles for the focused node, recomputed each frame and kept
                // so the next mouse press hit-tests exactly what was drawn.
                // No handles while the sim runs: they would move a node the
                // physics engine is about to move back.
                self.handles = match (self.session.is_some() && self.play.is_none(), self.focused()) {
                    (true, Some(path)) => self
                        .view
                        .node_bounds(&path)
                        .map(|b| {
                            let c = (Vec3::from_array(b.min) + Vec3::from_array(b.max)) * 0.5;
                            gizmo::handles(&projection, c)
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };

                // Before `drawn` borrows the object list: this prunes the
                // faded entries, so it needs `&mut self`.
                let marks = self.agent_marks(&projection, now);

                let drawn = match self.play.as_ref() {
                    Some(_) => &self.play_objects,
                    None => &self.view.objects,
                };

                // **Particles only run while the simulation does.** They used
                // to advance one tick per frame whether or not Play was on, so
                // opening a scene played its explosions before the human had
                // pressed anything — and because the editor drops its particle
                // state whenever the scene changes, every frame of a gizmo
                // drag started the blast over again.
                //
                // Not playing, the plume is built and left alone: warmed to
                // the settled population for a continuous emitter, which is
                // what `loom render` shows and what makes placing a chimney
                // possible, and empty for a one-shot, because a burst is an
                // event and opening a file is not one.
                //
                // Advanced rather than rebuilt, for the reason it always was:
                // re-simulating every plume's whole history each frame cost
                // 9.5 ms on this scene — four times a 67-million-voxel
                // terrain, for three props on a box.
                let world = match self.play.as_ref() {
                    Some(play) => &play.world,
                    None => self.view.world(),
                };
                let plumes = self
                    .plumes
                    .get_or_insert_with(|| crate::particles::Plumes::new(world));
                plumes.advance(stepped);
                let particles: &[loom_render::ParticleInstance] = plumes.instances();

                let mut actions = Vec::new();
                let state = PanelState {
                    agent_marks: &marks,
                    view: &self.view,
                    playing: self.play.as_ref().map(|p| (p.ticks, p.paused, p.bodies())),
                    selected: &self.selected,
                    history: self.session.as_ref().map_or(&[][..], |s| s.history()),
                    can_undo: self.session.as_ref().is_some_and(loom_scene::Session::can_undo),
                    can_redo: self.session.as_ref().is_some_and(loom_scene::Session::can_redo),
                    dirty: self.dirty,
                    conflict: self.conflict.is_some(),
                    editable: self.session.is_some(),
                    registry: &self.registry,
                    mode: self.mode,
                    handles: &self.handles,
                    dragging: self.drag.as_ref().map(|d| d.handle.axis),
                    fps: self.fps,
                };

                let result = match (self.viewer.as_mut(), self.ui.as_mut(), self.window.as_ref()) {
                    (Some(viewer), Some(ui), Some(window)) => viewer.draw_with_ui(
                        drawn,
                        particles,
                        &camera,
                        Some((ui, window)),
                        |root| actions.extend(crate::panels::draw(root, &state)),
                    ),
                    (Some(viewer), _, _) => viewer.draw(drawn, &camera),
                    _ => Ok(()),
                };
                if let Err(e) = result {
                    eprintln!("loom: draw failed: {e}");
                    event_loop.exit();
                }
                for action in actions {
                    self.act(action);
                }
                // Transitions are per-frame, so clearing them is what makes
                // `pressed` mean "this frame" rather than "ever".
                self.input.end_frame();

                if let Some(left) = self.frames_left {
                    let left = left.saturating_sub(1);
                    self.frames_left = Some(left);
                    if left == 0 {
                        crate::log::info("frame budget reached — closing");
                        self.shutdown(event_loop);
                        return;
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let DeviceEvent::MouseMotion { delta } = event else {
            return;
        };
        #[allow(clippy::cast_possible_truncation)]
        let (dx, dy) = (delta.0 as f32, delta.1 as f32);

        // Captured means the human is inside the scene, not looking at it.
        if self.captured {
            if let Some(play) = self.play.as_mut() {
                play.look(dx * LOOK_SENSITIVITY, dy * LOOK_SENSITIVITY);
            }
            return;
        }

        if self.looking() {
            self.camera.yaw -= dx * LOOK_SENSITIVITY;
            // Clamp just short of straight up/down: at exactly ±90° the
            // forward vector becomes parallel to world up and `right()`
            // degenerates, which makes strafing snap around.
            self.camera.pitch = (self.camera.pitch - dy * LOOK_SENSITIVITY)
                .clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
        }
    }
}

impl App {
    /// Apply one UI action. Every path that changes the scene funnels through
    /// `transact`, which funnels through `Session::apply` — one transaction
    /// path, one undo stack, whether the request came from a panel, a key, a
    /// gizmo, or the agent.
    fn act(&mut self, action: UiAction) {
        match action {
            UiAction::Select { path, extend } => self.select(&path, extend),
            UiAction::SetField(node, field, value) => self.set_field(&node, &field, value),
            UiAction::SetMode(mode) => self.mode = mode,
            UiAction::Focus => {
                self.camera = FlyCamera::framing_at(self.focus_bounds(), self.camera.fov_y_degrees);
            }
            UiAction::AddChild(parent) => self.add_child(&parent),
            UiAction::Duplicate => self.duplicate_selection(),
            UiAction::Delete => self.delete_selection(),
            UiAction::AssignMesh(asset) => self.assign_mesh(&asset),
            UiAction::ReloadFromDisk => self.accept_disk(),
            UiAction::KeepMine => self.keep_mine(),
            UiAction::ClearLog => crate::log::clear(),
            UiAction::Rename(node, name) => self.rename(&node, &name),
            UiAction::AddComponent(type_name) => self.add_component(&type_name),
            UiAction::RemoveComponent(node, type_name) => self.transact(
                format!("Remove {type_name} from {node}"),
                vec![loom_scene::SceneOp::RemoveComponent {
                    node,
                    component: type_name,
                }],
            ),
            UiAction::Reparent { node, parent } => self.reparent(&node, &parent),
            UiAction::Play => self.start_play(),
            UiAction::Pause => {
                if let Some(play) = self.play.as_mut() {
                    play.paused = !play.paused;
                }
            }
            UiAction::StepOnce => {
                if let Some(play) = self.play.as_mut() {
                    play.run(1);
                    self.refresh_play_objects();
                }
            }
            UiAction::Stop => self.stop_play(),
            // `dirty` only when something actually moved. Setting it on a
            // no-op Ctrl+Z latched it true forever — nothing clears it but a
            // save — so the viewport stopped following the file, every agent
            // write raised a conflict banner over edits that did not exist,
            // and "Keep mine" then wrote back the text as it was when the
            // editor opened. One reflexive undo could erase a whole session of
            // the agent's work.
            UiAction::Undo => {
                if let Some(s) = self.session.as_mut()
                    && s.undo()
                {
                    self.dirty = true;
                    self.resync();
                }
            }
            UiAction::Redo => {
                if let Some(s) = self.session.as_mut()
                    && s.redo()
                {
                    self.dirty = true;
                    self.resync();
                }
            }
            UiAction::Save => self.save(),
        }
    }

    /// Start simulating the scene as authored.
    ///
    /// The file is not touched, now or at Stop. Unity's oldest usability wound
    /// is edits made during play quietly vanishing when you stop; nothing here
    /// is at risk because nothing was written.
    fn start_play(&mut self) {
        let world = loom_ecs::World::from_scene(&self.view.scene);
        let play = crate::play::Play::start(world, &self.base);
        let (characters, scripts) = play.moving_parts();
        crate::log::info(format!(
            "play — {} bodies, {characters} characters, {scripts} scripts",
            play.bodies()
        ));
        let drivable = play.has_player() && play.camera().is_some();
        self.play = Some(play);
        // Effects restart with the simulation. Otherwise a one-shot that
        // already ran in a previous session of Play would stay burnt out, and
        // pressing Play twice would show the explosion only once.
        self.plumes = None;
        // First person only when the scene actually has the rig for it: a
        // character to move and a camera to see through. Without both, Play
        // stays a spectator view and the fly camera keeps working.
        if drivable {
            self.capture_pointer(true);
            crate::log::info("WASD to move, mouse to look, Space to jump, Esc to free the pointer");
        } else {
            crate::log::info(
                "no player rig (needs a CharacterController and a Camera) — flying instead",
            );
        }
        self.refresh_play_objects();
        self.drag = None;
    }

    /// Take or release the pointer for first-person play.
    ///
    /// Locked first, then Confined: Wayland and X11 disagree about which they
    /// support, and a viewer that panics on an unsupported grab mode is worse
    /// than one whose cursor leaves the window.
    fn capture_pointer(&mut self, capture: bool) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if capture {
            let locked = window
                .set_cursor_grab(winit::window::CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(winit::window::CursorGrabMode::Confined));
            if let Err(e) = locked {
                crate::log::warn(format!("could not capture the pointer: {e}"));
                return;
            }
        } else if window.set_cursor_grab(winit::window::CursorGrabMode::None).is_err() {
            return;
        }
        window.set_cursor_visible(!capture);
        self.captured = capture;
    }

    /// Sample this frame's keys for the character being driven.
    fn feed_play_input(&mut self) {
        let Some(play) = self.play.as_mut() else {
            return;
        };
        // Only while the pointer is captured. Otherwise typing in a panel
        // would walk the character around behind the human's back.
        if !self.captured {
            play.set_input(crate::play::PlayerInput::default());
            return;
        }
        play.set_input(crate::play::PlayerInput {
            move_axis: [
                self.input.axis(&self.bindings, PLAY, "move_right", "move_left"),
                self.input.axis(&self.bindings, PLAY, "move_forward", "move_back"),
            ],
            jump: self.input.is_active(&self.bindings, PLAY, "jump"),
            sprint: self.input.is_active(&self.bindings, PLAY, "sprint"),
            fire: self.input.is_active(&self.bindings, PLAY, "fire"),
        });
    }

    /// Stop, and go back to the authored scene exactly as it was.
    fn stop_play(&mut self) {
        self.capture_pointer(false);
        // Stop discards the simulated world, so the explosions in it are gone
        // too. Leaving the counter high would make the first shot of the next
        // run look like one already seen, and it would never be drawn.
        self.detonations_seen = 0;
        self.plumes = None;
        if let Some(play) = self.play.take() {
            crate::log::info(format!(
                "stopped after {} ticks ({:.2}s) — state {:016x}",
                play.ticks,
                play.seconds(),
                play.state_hash()
            ));
        }
        self.play_objects.clear();
        // The file may have moved while the sim was running and the watcher
        // was asleep; pick that up now rather than sitting on a stale view.
        #[allow(clippy::disallowed_methods)]
        {
            self.next_watch = std::time::Instant::now();
        }
    }

    /// Give the particle systems any explosion a script has just set off.
    fn spawn_new_detonations(&mut self) {
        let Some(play) = self.play.as_ref() else {
            return;
        };
        let fired = play.fired();
        if fired.len() <= self.detonations_seen {
            return;
        }
        let fresh: Vec<(u64, [f32; 3])> = fired[self.detonations_seen..]
            .iter()
            .map(|(tick, blast)| (*tick, blast.at))
            .collect();
        self.detonations_seen = fired.len();

        // Against the play world, because that is where the dormant prefab is
        // — the same scene, but the one the simulation is holding.
        let world = &play.world;
        if let Some(plumes) = self.plumes.as_mut() {
            for (tick, at) in fresh {
                plumes.detonate(world, at, tick);
            }
        }
    }

    /// Re-derive draw calls from the simulated world.
    fn refresh_play_objects(&mut self) {
        if let Some(play) = self.play.as_ref() {
            self.play_objects = self.view.objects_of(&play.world);
        }
    }

    /// Stop drawing, then leave.
    ///
    /// The X window can be gone before the event that says so reaches us, and
    /// egui asks the window for its size on every frame — winit turns the
    /// resulting `BadDrawable` into a panic. Dropping our reference to the
    /// window here means the redraw path has nothing to ask.
    fn shutdown(&mut self, event_loop: &ActiveEventLoop) {
        // Same order as `Drop`, and for the same reason.
        self.viewer = None;
        self.ui = None;
        self.window = None;
        event_loop.exit();
    }

    fn save(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match session.save() {
            Ok(()) => {
                // Record what we just wrote, or the watcher reads our own save
                // back as somebody else's edit a quarter second later.
                self.disk_seen = session.version().clone();
                self.dirty = false;
                self.conflict = None;
                crate::log::info(format!("saved {}", self.scene_path.display()));
            }
            // The file moved under us between watcher ticks. Both versions are
            // intact; raise the same banner the watcher would have, rather
            // than overwriting somebody else's work (never-do #15).
            Err(loom_scene::SaveRejected::Stale { current }) => {
                crate::log::warn("not saved: the scene changed on disk — choose which version to keep");
                self.disk_seen = loom_scene::VersionToken::of(&current);
                self.conflict = Some(current);
            }
            Err(e) => crate::log::error(format!("save failed: {e}")),
        }
    }

    /// Add a component to the selection, at the defaults its schema declares.
    ///
    /// The defaults come from the registry, not from a table here — a second
    /// list of what a component starts as is a second answer, and the schema
    /// is the one the validator uses.
    fn add_component(&mut self, type_name: &str) {
        let Some(schema) = self.registry.describe(type_name) else {
            crate::log::error(format!("no component type named {type_name}"));
            return;
        };
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        let mut ops = Vec::new();
        for node in self.selected.clone() {
            // A component whose schema declares no properties has nothing to
            // write, and is reported below rather than silently doing nothing.
            if let Some(properties) = properties {
                for (field, spec) in properties {
                    let Some(default) = spec.get("default") else {
                        continue;
                    };
                    ops.push(loom_scene::SceneOp::SetField {
                        node: node.clone(),
                        field: format!("{type_name}.{field}"),
                        value: default.clone(),
                    });
                }
            }
        }
        if ops.is_empty() {
            crate::log::warn(format!("{type_name} declares no defaults to write"));
            return;
        }
        self.transact(format!("Add {type_name}"), ops);
    }

    /// Rename a node, keeping the selection on it under its new path.
    fn rename(&mut self, node: &str, name: &str) {
        if name.is_empty() || name == node.rsplit('/').next().unwrap_or(node) {
            return;
        }
        let moved = match node.rsplit_once('/') {
            Some((parent, _)) => format!("{parent}/{name}"),
            None => name.to_owned(),
        };
        self.transact(
            format!("Rename {node} to {name}"),
            vec![loom_scene::SceneOp::RenameNode {
                node: node.to_owned(),
                name: name.to_owned(),
            }],
        );
        self.reselect(node, &moved);
    }

    /// Move a node under a new parent.
    fn reparent(&mut self, node: &str, parent: &str) {
        // A child inherits its parent's scale, so a non-uniformly scaled
        // parent squashes whatever you drop into it. The blockout fixture
        // carries a comment warning about exactly this; saying it at the
        // moment it happens is more use than saying it in a file.
        if let Some(t) = self.view.transform_of(parent) {
            let s = t.scale;
            if (s[0] - s[1]).abs() > 1e-4 || (s[1] - s[2]).abs() > 1e-4 {
                crate::log::warn(format!(
                    "{parent} is scaled {s:?} — children inherit that and will be squashed"
                ));
            }
        }
        let name = node.rsplit('/').next().unwrap_or(node);
        let moved = format!("{parent}/{name}");
        self.transact(
            format!("Move {node} into {parent}"),
            vec![loom_scene::SceneOp::ReparentNode {
                node: node.to_owned(),
                parent: parent.to_owned(),
            }],
        );
        self.reselect(node, &moved);
    }

    /// Follow a node whose path just changed — but only if the change landed.
    fn reselect(&mut self, from: &str, to: &str) {
        if !self.view.paths.iter().any(|p| p == to) {
            return;
        }
        for path in &mut self.selected {
            if path == from {
                *path = to.to_owned();
            } else if let Some(rest) = path.strip_prefix(&format!("{from}/")) {
                *path = format!("{to}/{rest}");
            }
        }
    }

    fn select(&mut self, path: &str, extend: bool) {
        if !extend {
            self.selected.clear();
            self.selected.push(path.to_owned());
            return;
        }
        if let Some(at) = self.selected.iter().position(|p| p == path) {
            self.selected.remove(at);
        } else {
            self.selected.push(path.to_owned());
        }
    }

    /// Run a transaction and re-derive the view from the result.
    ///
    /// The single write path. Everything the editor does arrives here, which is
    /// why a gizmo drag and an agent transaction are indistinguishable in the
    /// file and share one undo stack (never-do #16).
    fn transact(&mut self, label: String, ops: Vec<loom_scene::SceneOp>) {
        self.transact_as(label, ops, None);
    }

    /// As [`Self::transact`], but as one frame of a continuing gesture.
    ///
    /// A gizmo drag or a scrubbed slider fires every frame. Without this the
    /// undo stack fills with a thousand entries for one movement of the hand;
    /// with it, one gesture is one Ctrl+Z.
    fn transact_as(&mut self, label: String, ops: Vec<loom_scene::SceneOp>, gesture: Option<&str>) {
        if ops.is_empty() {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let transaction = loom_scene::Transaction {
            label,
            ops,
            dry_run: false,
            expect_version: None,
        };
        let outcome = match gesture {
            Some(gesture) => session.apply_coalescing(transaction, gesture),
            None => session.apply(transaction),
        };
        match outcome {
            Ok(_) => {
                self.dirty = true;
                self.resync();
            }
            // A dragged slider can leave the schema's range mid-drag; the
            // rejection is correct, and the console collapses the repeats.
            Err(e) if e.error == "would_produce_invalid_scene" => {
                crate::log::warn(format!("rejected: {}", e.constraint));
            }
            Err(e) if e.error == "stale_version" => {
                // §7.17: reload, never force, never merge.
                crate::log::warn("the scene moved underneath — reloading rather than overwriting");
                if let Some(session) = self.session.as_mut()
                    && let Err(e) = session.reload()
                {
                    crate::log::error(format!("reload failed: {e}"));
                    return;
                }
                self.resync();
            }
            Err(e) => crate::log::error(format!("{}: {}", e.error, e.constraint)),
        }
    }

    /// Set one field through a transaction.
    fn set_field(&mut self, node: &str, field: &str, value: serde_json::Value) {
        // `Transform.pos` is the sugar's desugared name; the op layer writes
        // the node key back out, so the file keeps its canonical form.
        let op = if let Some(component_field) = field.strip_prefix("Transform.") {
            let v: Vec<f32> = serde_json::from_value(value).unwrap_or_default();
            if v.len() != 3 {
                return;
            }
            let axis = [v[0], v[1], v[2]];
            match component_field {
                "pos" => transform_op(node, Some(axis), None, None),
                "rot_euler" => transform_op(node, None, Some(axis), None),
                _ => transform_op(node, None, None, Some(axis)),
            }
        } else {
            loom_scene::SceneOp::SetField {
                node: node.to_owned(),
                field: field.to_owned(),
                value,
            }
        };
        // Scrubbing a DragValue fires per frame, exactly like a gizmo drag.
        let gesture = format!("field:{node}:{field}:{}", self.gesture_epoch);
        self.transact_as(format!("Set {node} {field}"), vec![op], Some(&gesture));
    }

    /// Add an empty child under `parent`, named so it does not collide.
    fn add_child(&mut self, parent: &str) {
        let mut name = "Node".to_owned();
        let mut n = 1;
        while self
            .view
            .paths
            .iter()
            .any(|p| p == &format!("{parent}/{name}"))
        {
            n += 1;
            name = format!("Node{n}");
        }
        self.transact(
            format!("Add {parent}/{name}"),
            vec![loom_scene::SceneOp::SpawnNode {
                parent: parent.to_owned(),
                name: name.clone(),
                mesh: Some("box".to_owned()),
            }],
        );
        self.selected = vec![format!("{parent}/{name}")];
    }

    /// Copy the selection, offset a little so the copy is visible.
    ///
    /// Built out of the ops that already exist rather than a `DuplicateNode`
    /// op: spawn, then set what the original had. One transaction, so it is
    /// still one Ctrl+Z.
    fn duplicate_selection(&mut self) {
        let mut ops = Vec::new();
        let mut created = Vec::new();
        for path in self.selected.clone() {
            let Some(node) = self.view.scene.nodes().iter().find(|n| n.path == path) else {
                continue;
            };
            let (parent, name) = match path.rsplit_once('/') {
                Some((parent, name)) => (parent.to_owned(), name.to_owned()),
                // A root node has nowhere to be a sibling of.
                None => {
                    crate::log::warn(format!("{path} is a root node; nothing to duplicate it into"));
                    continue;
                }
            };
            let mut copy = format!("{name}Copy");
            let mut n = 1;
            while self.view.paths.iter().any(|p| p == &format!("{parent}/{copy}")) {
                n += 1;
                copy = format!("{name}Copy{n}");
            }
            let new_path = format!("{parent}/{copy}");

            ops.push(loom_scene::SceneOp::SpawnNode {
                parent: parent.clone(),
                name: copy.clone(),
                mesh: None,
            });
            let t = &node.transform;
            ops.push(transform_op(
                &new_path,
                Some([t.pos[0] + 1.0, t.pos[1], t.pos[2]]),
                Some(t.rot_euler),
                Some(t.scale),
            ));
            for (type_name, value) in &node.components {
                if let Some(fields) = value.as_object() {
                    for (field, v) in fields {
                        ops.push(loom_scene::SceneOp::SetField {
                            node: new_path.clone(),
                            field: format!("{type_name}.{field}"),
                            value: v.clone(),
                        });
                    }
                }
            }
            created.push(new_path);
        }
        if created.is_empty() {
            return;
        }
        self.transact(format!("Duplicate {} node(s)", created.len()), ops);
        self.selected = created;
    }

    /// Delete the selection, children first.
    ///
    /// Deepest-first because removing a parent that still has children is
    /// refused — correctly, since it would produce an unloadable scene. Sorting
    /// here means the human does not have to.
    fn delete_selection(&mut self) {
        let mut doomed: Vec<String> = Vec::new();
        for path in &self.selected {
            let prefix = format!("{path}/");
            for candidate in &self.view.paths {
                if candidate == path || candidate.starts_with(&prefix) {
                    doomed.push(candidate.clone());
                }
            }
        }
        doomed.sort_by_key(|p| std::cmp::Reverse(p.matches('/').count()));
        doomed.dedup();
        if doomed.is_empty() {
            return;
        }
        let label = format!("Delete {} node(s)", doomed.len());
        let ops = doomed
            .into_iter()
            .map(|node| loom_scene::SceneOp::RemoveNode { node })
            .collect();
        self.selected.clear();
        self.transact(label, ops);
    }

    /// Point the selection's `MeshRenderer` at an asset.
    fn assign_mesh(&mut self, asset: &str) {
        let ops: Vec<_> = self
            .selected
            .iter()
            .map(|node| loom_scene::SceneOp::SetField {
                node: node.clone(),
                field: "MeshRenderer.mesh".to_owned(),
                value: serde_json::json!({ "asset": asset }),
            })
            .collect();
        self.transact(format!("Set mesh to {asset}"), ops);
    }

    /// A left press in the viewport: grab a handle if one is under the cursor,
    /// otherwise select whatever is.
    fn press_in_viewport(&mut self) {
        if self.session.is_none() {
            return;
        }
        if let Some(index) = gizmo::grab(&self.handles, self.cursor) {
            let Some(node) = self.focused() else { return };
            let Some(transform) = self.view.transform_of(&node) else {
                return;
            };
            self.drag = Some(Drag {
                handle: self.handles[index].clone(),
                from: self.cursor,
                start: [transform.pos, transform.rot_euler, transform.scale],
                node,
            });
            return;
        }
        self.pick_at_cursor();
    }

    /// Continue a gizmo drag.
    fn drag_gizmo(&mut self) {
        let Some(drag) = self.drag.as_ref() else {
            return;
        };
        // Absolute from the drag's start, not accumulated: a dropped frame then
        // costs nothing instead of leaving the node permanently off. The same
        // function the gizmo tests exercise — a second copy of this arithmetic
        // is a second answer, and only one of them would be under test.
        let travelled = gizmo::drag_distance(&drag.handle, drag.from, self.cursor);
        if travelled == 0.0 {
            return;
        }

        let (axis, node) = (drag.handle.axis, drag.node.clone());
        let (start_pos, start_rot, start_scale) = (drag.start[0], drag.start[1], drag.start[2]);
        let (label, op) = match self.mode {
            Mode::Move => {
                // The handle points along a **world** axis; the node stores a
                // **local** transform. Adding the travel straight to the local
                // component is only right at the root — under a turned or
                // scaled parent the node moved in the wrong direction and by
                // the wrong amount. Rotating the world delta by the parent's
                // inverse (direction only, hence `transform_vector3`) puts it
                // in the space the file is written in.
                let mut world_delta = loom_render::glam::Vec3::ZERO;
                world_delta[axis] = travelled;
                let local_delta = self
                    .view
                    .parent_inverse(&node)
                    .transform_vector3(world_delta);

                let pos = [
                    start_pos[0] + local_delta.x,
                    start_pos[1] + local_delta.y,
                    start_pos[2] + local_delta.z,
                ];
                (format!("Move {node}"), transform_op(&node, Some(pos), None, None))
            }
            Mode::Rotate => {
                let mut rot = start_rot;
                rot[axis] += travelled * ROTATE_PER_UNIT;
                (
                    format!("Rotate {node}"),
                    transform_op(&node, None, Some(rot), None),
                )
            }
            Mode::Scale => {
                let mut scale = start_scale;
                // Additive, not multiplicative: a scale of zero would otherwise
                // be a trap you cannot drag back out of.
                scale[axis] = (start_scale[axis] + travelled).max(0.01);
                (
                    format!("Scale {node}"),
                    transform_op(&node, None, None, Some(scale)),
                )
            }
        };
        let gesture = format!("gizmo:{node}:{axis}:{}", self.gesture_epoch);
        self.transact_as(label, vec![op], Some(&gesture));
    }

    /// Select whatever the cursor is over.
    ///
    /// `ponytail:` ray against node AABBs, not a GPU ID buffer. Pixel-perfect
    /// picking needs a second pass writing entity ids and a readback; an AABB
    /// test is thirty lines and is right for a blockout editor where nodes are
    /// boxes. Upgrade when picking a thin or concave mesh matters.
    fn pick_at_cursor(&mut self) {
        let Some(viewer) = self.viewer.as_ref() else {
            return;
        };
        let (w, h) = viewer.extent();
        #[allow(clippy::cast_precision_loss)]
        let projection = gizmo::View::new(&self.camera.camera(), w as f32, h as f32);
        let dir = projection.ray(self.cursor.0, self.cursor.1);

        let mut best: Option<(f32, &String)> = None;
        for (path, bounds) in &self.view.picks {
            if let Some(t) = ray_box(projection.eye(), dir, bounds)
                && best.is_none_or(|(d, _)| t < d)
            {
                best = Some((t, path));
            }
        }

        let extend = self.input.is_active(&self.bindings, EDIT, "extend");
        match best.map(|(_, path)| path.clone()) {
            Some(path) => {
                self.select(&path, extend);
                crate::log::info(format!("selected {path}"));
            }
            // Clicking empty space clears, the way every editor does.
            None if !extend => self.selected.clear(),
            None => {}
        }
    }

    /// Editing actions bound to keys.
    fn handle_editing(&mut self) {
        if self.session.is_none() || self.view.paths.is_empty() {
            return;
        }
        // Play mode shows the simulation, not the file. Editing the authored
        // scene from behind that — Delete, Duplicate, nudge, Undo, Save — was
        // reachable by keyboard and invisible, because the viewport was
        // drawing the simulated world. The toolbar already refuses; the keys
        // did not.
        if self.play.is_some() {
            return;
        }
        // Sampled up front rather than through a closure borrowing `self`,
        // because the handlers below need `&mut self`.
        let act = |a: &str| self.input.is_active(&self.bindings, EDIT, a);
        let (next, prev) = (act("select_next"), act("select_prev"));
        let delta = Vec3::new(
            f32::from(act("nudge_right")) - f32::from(act("nudge_left")),
            f32::from(act("nudge_up")) - f32::from(act("nudge_down")),
            f32::from(act("nudge_back")) - f32::from(act("nudge_forward")),
        ) * NUDGE;
        let (undo, redo, save) = (act("undo"), act("redo"), act("save"));
        let (duplicate, delete) = (act("duplicate"), act("delete"));
        let mode = if act("mode_move") {
            Some(Mode::Move)
        } else if act("mode_rotate") {
            Some(Mode::Rotate)
        } else if act("mode_scale") {
            Some(Mode::Scale)
        } else {
            None
        };

        if next || prev {
            self.step_selection(next);
        }
        if let Some(mode) = mode {
            self.mode = mode;
            crate::log::info(format!("{} mode", mode.label()));
        }
        if delta != Vec3::ZERO {
            self.nudge(delta);
        }
        if duplicate {
            self.duplicate_selection();
        }
        if delete {
            self.delete_selection();
        }
        if undo {
            self.act(UiAction::Undo);
        }
        if redo {
            self.act(UiAction::Redo);
        }
        if save {
            self.save();
        }
    }

    /// Tab through the hierarchy.
    fn step_selection(&mut self, forward: bool) {
        let paths = &self.view.paths;
        if paths.is_empty() {
            return;
        }
        let current = self
            .focused()
            .and_then(|p| paths.iter().position(|c| c == &p))
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % paths.len()
        } else {
            (current + paths.len() - 1) % paths.len()
        };
        self.selected = vec![paths[next].clone()];
        crate::log::info(format!("selected {}", paths[next]));
    }

    /// Move the selection by issuing one transaction for all of it.
    fn nudge(&mut self, delta: Vec3) {
        let mut ops = Vec::new();
        for path in self.selected.clone() {
            let Some(current) = self.view.transform_of(&path).map(|t| t.pos) else {
                continue;
            };
            ops.push(transform_op(
                &path,
                Some([
                    current[0] + delta.x,
                    current[1] + delta.y,
                    current[2] + delta.z,
                ]),
                None,
                None,
            ));
        }
        // Labelled usefully: this shows up in the human's log panel and in git
        // history. "Move Room/Desk" beats "update scene".
        let label = match self.focused() {
            Some(node) => format!("Move {node}"),
            None => format!("Move {} nodes", ops.len()),
        };
        self.transact(label, ops);
    }
}

/// A `SetTransform` op. Omitted fields are left alone by the op layer.
fn transform_op(
    node: &str,
    pos: Option<[f32; 3]>,
    rot_euler: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
) -> loom_scene::SceneOp {
    loom_scene::SceneOp::SetTransform {
        node: node.to_owned(),
        pos,
        rot_euler,
        scale,
    }
}

/// Slab test: distance along `dir` where the ray enters the box, if it does.
fn ray_box(origin: Vec3, dir: Vec3, bounds: &loom_scene::place::Bounds) -> Option<f32> {
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    for axis in 0..3 {
        let d = dir[axis];
        let (lo, hi) = (bounds.min[axis], bounds.max[axis]);
        if d.abs() < 1e-6 {
            // Parallel to this slab: a miss unless the origin is already inside.
            if origin[axis] < lo || origin[axis] > hi {
                return None;
            }
            continue;
        }
        let t0 = (lo - origin[axis]) / d;
        let t1 = (hi - origin[axis]) / d;
        let (t0, t1) = if t0 > t1 { (t1, t0) } else { (t0, t1) };
        near = near.max(t0);
        far = far.min(t1);
        if near > far {
            return None;
        }
    }
    (far > 0.0).then(|| near.max(0.0))
}

fn build_viewer(
    window: &Arc<Window>,
    meshes: &[loom_asset::Mesh],
    textures: &[loom_asset::Texture],
    materials: &[loom_render::MaterialData],
) -> Result<(Instance, Device, Viewer), String> {
    use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

    let display = window
        .display_handle()
        .map_err(|e| format!("no display handle: {e}"))?
        .as_raw();
    let window_handle = window
        .window_handle()
        .map_err(|e| format!("no window handle: {e}"))?
        .as_raw();

    let required = ash_window::enumerate_required_extensions(display)
        .map_err(|e| format!("surface extensions unavailable: {e}"))?;
    let instance = Instance::with_extensions(c"loom", required).map_err(|e| e.to_string())?;

    // SAFETY: the window outlives the surface. `App`'s `Drop` destroys the
    // viewer — and with it this surface — before releasing the window. That is
    // written out explicitly there rather than left to field order, because
    // field order is what got this wrong the first time.
    let surface = unsafe {
        ash_window::create_surface(
            instance.entry(),
            instance.handle(),
            display,
            window_handle,
            None,
        )
    }
    .map_err(|e| format!("could not create a surface: {e:?}"))?;

    let surface_loader = ash::khr::surface::Instance::new(instance.entry(), instance.handle());
    let device =
        Device::for_surface(&instance, (&surface_loader, surface)).map_err(|e| e.to_string())?;

    let size = window.inner_size();
    let viewer = Viewer::new(
        &instance,
        &device,
        surface,
        size.width,
        size.height,
        meshes,
        textures,
        materials,
    )
    .map_err(|e| e.to_string())?;

    Ok((instance, device, viewer))
}

/// Bindings from `assets/input/default.toml` if present, else the compiled-in
/// copy of the same file.
///
/// The on-disk file wins so rebinding needs no rebuild — which is the point of
/// M6. A malformed file is reported and then ignored rather than being fatal:
/// losing your camera because of a typo in a config is a bad trade.
fn load_bindings() -> ActionMap {
    let path = std::path::Path::new("assets/input/default.toml");
    if path.exists() {
        match ActionMap::load(path) {
            Ok(map) => return map,
            Err(e) => eprintln!("loom: {}: {e}; using built-in bindings", path.display()),
        }
    }
    ActionMap::from_toml(loom_input::DEFAULT_BINDINGS).unwrap_or_default()
}

/// Open a window showing `path`.
///
/// # Errors
/// A message describing what stopped it.
pub fn run(
    path: &str,
    view: SceneView,
    session: Option<loom_scene::Session>,
    disk_seen: loom_scene::VersionToken,
    frames: Option<u32>,
) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("no event loop: {e}"))?;
    // Poll, not Wait: the camera animates continuously while keys are held, and
    // the file watcher has to keep ticking whether or not the human touches
    // anything — a window that only redraws on input would never show the
    // agent's writes.
    event_loop.set_control_flow(ControlFlow::Poll);

    let title = format!("loom — {path}");
    let mut app = App::new(
        view,
        title,
        session,
        std::path::PathBuf::from(path),
        disk_seen,
    );
    app.frames_left = frames.filter(|n| *n > 0);
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop failed: {e}"))
}

/// Load a scene and open it in a window.
///
/// # Errors
/// A message describing what stopped it.
pub fn open_scene(path: &str, editable: bool, frames: Option<u32>) -> Result<(), String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let base = std::path::Path::new(path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let view = SceneView::build(&src, base)?;
    let disk_seen = loom_scene::VersionToken::of(&src);

    // Read-only unless asked. A viewer that cannot write cannot race the
    // agent's writes at all — but it still follows them.
    let session = editable
        .then(|| loom_scene::Session::open(std::path::Path::new(path)))
        .transpose()
        .map_err(|e| format!("{path}: {e}"))?;

    run(path, view, session, disk_seen, frames)
}
