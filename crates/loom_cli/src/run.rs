//! `loom run` — a window showing the scene, with a fly camera.
//!
//! Read-only by design. Brief §2 puts the editable editor at M12 and a
//! **read-only** viewer at M5.5; this is the camera half of that, early. It
//! cannot modify the scene, so it cannot race the agent's writes — the
//! split-brain problem §7.17 is about does not exist until editing does.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use loom_ecs::World;
use loom_input::{ActionMap, InputState};
use loom_render::glam::Vec3;
use loom_render::{Camera, Device, Instance, Object, Viewer, ash, ash_window};
use loom_scene::Scene;
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

/// An orbit-free fly camera: position plus yaw/pitch.
struct FlyCamera {
    position: Vec3,
    yaw: f32,
    pitch: f32,
}

impl FlyCamera {
    /// Frame the scene, then look at its centre — so the window opens on the
    /// content rather than on empty space.
    fn framing(bounds: (Vec3, f32)) -> Self {
        let (center, radius) = bounds;
        let distance = (radius * 2.2).max(4.0);
        let position = center + Vec3::new(0.6, 0.45, 1.0).normalize() * distance;

        let to_center = (center - position).normalize_or_zero();
        Self {
            position,
            yaw: to_center.x.atan2(to_center.z),
            pitch: to_center.y.asin(),
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
            fov_y_degrees: 60.0,
        }
    }
}

/// The context the viewer runs in. A menu would push another.
const FLY: &str = "fly";
/// Editing actions, live only when `--edit` was passed.
const EDIT: &str = "edit";
/// How far one nudge moves a node, in metres.
const NUDGE: f32 = 0.25;

struct App {
    objects: Vec<Object>,
    /// `Some` when `--edit` was passed. Read-only otherwise, which is the
    /// default because a viewer that cannot write cannot race the agent.
    session: Option<loom_scene::Session>,
    /// Index into `editable` of the selected node.
    selected: usize,
    /// Scene paths that can be moved, in scene order.
    editable: Vec<String>,
    /// Where the scene lives, for reloading after a rejected write.
    scene_path: std::path::PathBuf,
    /// Real world bounds, so framing does not assume unit cubes.
    scene_bounds: (Vec3, f32),
    meshes: Vec<loom_asset::Mesh>,
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
    title: String,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    fn new(
        objects: Vec<Object>,
        meshes: Vec<loom_asset::Mesh>,
        scene_bounds: (Vec3, f32),
        title: String,
        session: Option<loom_scene::Session>,
        editable: Vec<String>,
        scene_path: std::path::PathBuf,
    ) -> Self {
        Self {
            camera: FlyCamera::framing(scene_bounds),
            scene_bounds,
            objects,
            session,
            selected: 0,
            editable,
            scene_path,
            meshes,
            // Prefer a project-local file, fall back to the shipped defaults,
            // so a fresh checkout has a working camera with no config to write.
            bindings: load_bindings(),
            input: InputState::new(),
            window: None,
            viewer: None,
            gpu: None,
            // `clippy.toml` disallows Instant::now, and it is right to fire.
            // The rule is that SIMULATION must not read the wall clock
            // (never-do #8, §7.5) — a deterministic tick cannot depend on how
            // fast the machine is. This is the presentation loop: frame pacing
            // for a camera a human is flying, which is exactly what wall time
            // is for. Scoped to these two reads rather than weakening the lint.
            #[allow(clippy::disallowed_methods)]
            last_frame: std::time::Instant::now(),
            title,
        }
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
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("loom: could not create a window: {e}");
                event_loop.exit();
                return;
            }
        };

        match build_viewer(&window, &self.meshes) {
            Ok((instance, device, viewer)) => {
                eprintln!("loom: rendering on {}", device.name());
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            // Keys are recorded by NAME and interpreted by the action map.
            // Nothing here knows what W means.
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.input
                        .set_button(&format!("{code:?}"), event.state == ElementState::Pressed);
                }
                if self.input.is_active(&self.bindings, FLY, "quit") {
                    event_loop.exit();
                }
                if self.input.is_active(&self.bindings, FLY, "reframe") {
                    self.camera = FlyCamera::framing(self.scene_bounds);
                }
                self.handle_editing();
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let name = match button {
                    MouseButton::Left => "MouseLeft",
                    MouseButton::Right => "MouseRight",
                    MouseButton::Middle => "MouseMiddle",
                    _ => return,
                };
                self.input.set_button(name, state == ElementState::Pressed);
            }

            WindowEvent::Resized(_) => {
                if let Some(viewer) = self.viewer.as_mut()
                    && let Err(e) = viewer.recreate()
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
                // Clamp: a stall must not teleport the camera across the map.
                self.step_camera(dt.min(0.1));

                if let Some(viewer) = self.viewer.as_mut()
                    && let Err(e) = viewer.draw(&self.objects, &self.camera.camera())
                {
                    eprintln!("loom: draw failed: {e}");
                    event_loop.exit();
                }
                // Transitions are per-frame, so clearing them is what makes
                // `pressed` mean "this frame" rather than "ever".
                self.input.end_frame();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.looking()
        {
            #[allow(clippy::cast_possible_truncation)]
            let (dx, dy) = (delta.0 as f32, delta.1 as f32);
            self.camera.yaw -= dx * LOOK_SENSITIVITY;
            // Clamp just short of straight up/down: at exactly ±90° the
            // forward vector becomes parallel to world up and `right()`
            // degenerates, which makes strafing snap around.
            self.camera.pitch =
                (self.camera.pitch - dy * LOOK_SENSITIVITY).clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
        }
    }
}

impl App {
    /// Editing actions, when a session is open.
    ///
    /// Every change goes through `Session::apply`, which is the same
    /// `loom_scene::apply` the agent calls — so a human edit and an agent edit
    /// produce the same bytes and share one undo stack (never-do #16).
    fn handle_editing(&mut self) {
        if self.session.is_none() || self.editable.is_empty() {
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

        if next {
            self.selected = (self.selected + 1) % self.editable.len();
            eprintln!("loom: selected {}", self.editable[self.selected]);
        }
        if prev {
            self.selected = (self.selected + self.editable.len() - 1) % self.editable.len();
            eprintln!("loom: selected {}", self.editable[self.selected]);
        }
        if delta != Vec3::ZERO {
            self.nudge(delta);
        }
        if undo {
            let session = self.session.as_mut().expect("checked above");
            eprintln!("loom: undo {}", if session.undo() { "ok" } else { "(nothing)" });
        }
        if redo {
            let session = self.session.as_mut().expect("checked above");
            eprintln!("loom: redo {}", if session.redo() { "ok" } else { "(nothing)" });
        }
        if save {
            let session = self.session.as_ref().expect("checked above");
            match session.save() {
                Ok(()) => eprintln!("loom: saved {}", self.scene_path.display()),
                Err(e) => eprintln!("loom: save failed: {e}"),
            }
        }
    }

    /// Move the selected node by issuing a transaction.
    fn nudge(&mut self, delta: Vec3) {
        let node = self.editable[self.selected].clone();
        let Some(session) = self.session.as_mut() else {
            return;
        };

        // Read the current transform from the scene rather than tracking it
        // separately — a second copy of the truth is a second answer.
        let Ok(scene) = loom_scene::Scene::parse(session.text()) else {
            return;
        };
        let Some(current) = scene
            .nodes()
            .iter()
            .find(|n| n.path == node)
            .map(|n| n.transform.pos)
        else {
            return;
        };
        let pos = [
            current[0] + delta.x,
            current[1] + delta.y,
            current[2] + delta.z,
        ];

        let transaction = loom_scene::Transaction {
            // Labelled usefully: this shows up in the human's log panel and in
            // git history. "Move Room/Desk" beats "update scene".
            label: format!("Move {node}"),
            ops: vec![loom_scene::SceneOp::SetTransform {
                node: node.clone(),
                pos: Some(pos),
                rot_euler: None,
                scale: None,
            }],
            dry_run: false,
            expect_version: None,
        };

        match session.apply(transaction) {
            Ok(_) => {}
            Err(e) if e.error == "stale_version" => {
                // §7.17: reload, never force, never merge.
                eprintln!("loom: the scene changed on disk — reloading rather than overwriting");
                if let Err(e) = session.reload() {
                    eprintln!("loom: reload failed: {e}");
                }
            }
            Err(e) => eprintln!("loom: {e}"),
        }
    }
}

fn build_viewer(
    window: &Arc<Window>,
    meshes: &[loom_asset::Mesh],
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

    // SAFETY: the window outlives the surface — both are owned by `App`, and
    // the viewer is dropped before the window.
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
    let device = Device::for_surface(&instance, (&surface_loader, surface))
        .map_err(|e| e.to_string())?;

    let size = window.inner_size();
    let viewer = Viewer::new(&instance, &device, surface, size.width, size.height, meshes)
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
#[allow(clippy::too_many_arguments)]
pub fn run(
    path: &str,
    objects: Vec<Object>,
    meshes: Vec<loom_asset::Mesh>,
    scene_bounds: (Vec3, f32),
    session: Option<loom_scene::Session>,
    editable: Vec<String>,
) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("no event loop: {e}"))?;
    // Poll, not Wait: the camera animates continuously while keys are held.
    event_loop.set_control_flow(ControlFlow::Poll);

    let title = format!("loom — {path}");
    let mut app = App::new(
        objects,
        meshes,
        scene_bounds,
        title,
        session,
        editable,
        std::path::PathBuf::from(path),
    );
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop failed: {e}"))
}

/// Load a scene and open it in a window.
///
/// # Errors
/// A message describing what stopped it.
pub fn open_scene(path: &str, editable: bool) -> Result<(), String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let scene = Scene::parse(&src).map_err(|errors| {
        serde_json::to_string_pretty(&serde_json::json!({ "errors": errors }))
            .unwrap_or_else(|_| "invalid scene".to_owned())
    })?;
    let world = World::from_scene(&scene);
    let base = std::path::Path::new(path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let library = crate::MeshLibrary::for_scene(&scene, base);
    let objects = crate::world_to_objects(&world, &library);
    let boxes = crate::node_bounds(&world, &library);
    let scene_bounds = crate::scene_bounds(&boxes);

    // Read-only unless asked. Brief §2 puts the editable editor at M12 and a
    // read-only viewer at M5.5, and a viewer that cannot write cannot race the
    // agent's writes at all.
    let session = editable
        .then(|| loom_scene::Session::open(std::path::Path::new(path)))
        .transpose()
        .map_err(|e| format!("{path}: {e}"))?;
    let names: Vec<String> = scene.nodes().iter().map(|n| n.path.clone()).collect();

    run(path, objects, library.into_meshes(), scene_bounds, session, names)
}
