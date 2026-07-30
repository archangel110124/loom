//! `loom run` — a window showing the scene, with a fly camera.
//!
//! Read-only by design. Brief §2 puts the editable editor at M12 and a
//! **read-only** viewer at M5.5; this is the camera half of that, early. It
//! cannot modify the scene, so it cannot race the agent's writes — the
//! split-brain problem §7.17 is about does not exist until editing does.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use loom_ecs::World;
use loom_render::glam::Vec3;
use loom_render::{Camera, Device, Instance, Object, Viewer, ash, ash_window};
use loom_scene::Scene;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
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

/// Which movement keys are currently held.
#[derive(Default)]
struct Keys {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    sprint: bool,
}

struct App {
    objects: Vec<Object>,
    camera: FlyCamera,
    keys: Keys,
    looking: bool,
    window: Option<Arc<Window>>,
    viewer: Option<Viewer>,
    /// Kept alive for the whole session: destroying the device before the
    /// viewer's resources would be a use-after-free.
    gpu: Option<(Instance, Device)>,
    last_frame: std::time::Instant,
    title: String,
}

impl App {
    fn new(objects: Vec<Object>, title: String) -> Self {
        Self {
            camera: FlyCamera::framing(&objects),
            objects,
            keys: Keys::default(),
            looking: false,
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
        let speed = if self.keys.sprint {
            MOVE_SPEED * SPRINT_MULTIPLIER
        } else {
            MOVE_SPEED
        } * dt;

        let forward = self.camera.forward();
        let right = self.camera.right();
        let mut delta = Vec3::ZERO;
        if self.keys.forward {
            delta += forward;
        }
        if self.keys.back {
            delta -= forward;
        }
        if self.keys.right {
            delta += right;
        }
        if self.keys.left {
            delta -= right;
        }
        // World up, not camera up: holding E should rise vertically even when
        // looking down, which is what every fly camera does.
        if self.keys.up {
            delta += Vec3::Y;
        }
        if self.keys.down {
            delta -= Vec3::Y;
        }

        self.camera.position += delta.normalize_or_zero() * speed;
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

        match build_viewer(&window) {
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

            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) => event_loop.exit(),
                    PhysicalKey::Code(KeyCode::KeyW) => self.keys.forward = pressed,
                    PhysicalKey::Code(KeyCode::KeyS) => self.keys.back = pressed,
                    PhysicalKey::Code(KeyCode::KeyA) => self.keys.left = pressed,
                    PhysicalKey::Code(KeyCode::KeyD) => self.keys.right = pressed,
                    PhysicalKey::Code(KeyCode::KeyE | KeyCode::Space) => self.keys.up = pressed,
                    PhysicalKey::Code(KeyCode::KeyQ) => self.keys.down = pressed,
                    PhysicalKey::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight) => {
                        self.keys.sprint = pressed;
                    }
                    PhysicalKey::Code(KeyCode::KeyF) if pressed => {
                        self.camera = FlyCamera::framing(&self.objects);
                    }
                    _ => {}
                }
            }

            // Hold a mouse button to look, so the cursor stays usable
            // otherwise — this is a viewer, not a game.
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right | MouseButton::Left,
                ..
            } => {
                self.looking = state == ElementState::Pressed;
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
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.looking
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

fn build_viewer(window: &Arc<Window>) -> Result<(Instance, Device, Viewer), String> {
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
    let viewer = Viewer::new(&instance, &device, surface, size.width, size.height)
        .map_err(|e| e.to_string())?;

    Ok((instance, device, viewer))
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
pub fn run(path: &str, objects: Vec<Object>) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("no event loop: {e}"))?;
    // Poll, not Wait: the camera animates continuously while keys are held.
    event_loop.set_control_flow(ControlFlow::Poll);

    let title = format!("loom — {path}");
    let mut app = App::new(objects, title);
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop failed: {e}"))
}

/// Load a scene and open it in a window.
///
/// # Errors
/// A message describing what stopped it.
pub fn open_scene(path: &str, to_objects: impl Fn(&World) -> Vec<Object>) -> Result<(), String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let scene = Scene::parse(&src).map_err(|errors| {
        serde_json::to_string_pretty(&serde_json::json!({ "errors": errors }))
            .unwrap_or_else(|_| "invalid scene".to_owned())
    })?;
    let world = World::from_scene(&scene);
    run(path, to_objects(&world))
}
