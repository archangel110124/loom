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
    fn framing(objects: &[Object]) -> Self {
        let (center, radius) = bounds(objects);
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

struct App {
    objects: Vec<Object>,
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
    fn new(objects: Vec<Object>, meshes: Vec<loom_asset::Mesh>, title: String) -> Self {
        Self {
            camera: FlyCamera::framing(&objects),
            objects,
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
                    self.camera = FlyCamera::framing(&self.objects);
                }
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

fn bounds(objects: &[Object]) -> (Vec3, f32) {
    if objects.is_empty() {
        return (Vec3::ZERO, 4.0);
    }
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for object in objects {
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { -1.0 } else { 1.0 },
            );
            let p = object.model.transform_point3(corner);
            min = min.min(p);
            max = max.max(p);
        }
    }
    let center = (min + max) * 0.5;
    (center, (max - min).length() * 0.5)
}

/// Open a window showing `path`.
///
/// # Errors
/// A message describing what stopped it.
pub fn run(path: &str, objects: Vec<Object>, meshes: Vec<loom_asset::Mesh>) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("no event loop: {e}"))?;
    // Poll, not Wait: the camera animates continuously while keys are held.
    event_loop.set_control_flow(ControlFlow::Poll);

    let title = format!("loom — {path}");
    let mut app = App::new(objects, meshes, title);
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop failed: {e}"))
}

/// Load a scene and open it in a window.
///
/// # Errors
/// A message describing what stopped it.
pub fn open_scene(path: &str) -> Result<(), String> {
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
    run(path, objects, library.into_meshes())
}
