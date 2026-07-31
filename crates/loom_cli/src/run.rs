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
use loom_render::{Camera, Device, Instance, Object, Ui, Viewer, ash, ash_window};

use crate::panels::{PanelState, UiAction};
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
    /// egui, when editing. `None` in read-only mode, so a viewer costs no UI.
    ui: Option<Ui>,
    /// World bounds per node, for click-to-select.
    picks: std::collections::BTreeMap<String, loom_scene::place::Bounds>,
    /// Cursor position, for picking.
    cursor: (f32, f32),
    registry: loom_reflect::TypeRegistry,
    dirty: bool,
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
        picks: std::collections::BTreeMap<String, loom_scene::place::Bounds>,
    ) -> Self {
        Self {
            camera: FlyCamera::framing(scene_bounds),
            scene_bounds,
            objects,
            session,
            selected: 0,
            editable,
            scene_path,
            ui: None,
            picks,
            cursor: (0.0, 0.0),
            registry: loom_scene::components::registry(),
            dirty: false,
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
                if self.session.is_some() {
                    match Ui::new(&instance, &device, &window, viewer.color_format()) {
                        Ok(ui) => self.ui = Some(ui),
                        Err(e) => eprintln!("loom: no editor UI ({e}); continuing read-only"),
                    }
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
        // egui sees every event first. If it consumed one, the viewport must
        // NOT also act on it — otherwise clicking a panel also moves the camera
        // behind it and typing a number in the inspector flies the camera.
        let consumed = match (self.ui.as_mut(), self.window.as_ref()) {
            (Some(ui), Some(window)) => ui.on_window_event(window, &event),
            _ => false,
        };
        if consumed && !matches!(event, WindowEvent::RedrawRequested | WindowEvent::Resized(_)) {
            return;
        }

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

            // Click to select, when editing and not over a panel.
            WindowEvent::CursorMoved { position, .. } => {
                #[allow(clippy::cast_possible_truncation)]
                {
                    self.cursor = (position.x as f32, position.y as f32);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left
                    && state == ElementState::Pressed
                    && self.session.is_some()
                    && !self.ui.as_ref().is_some_and(Ui::wants_pointer)
                {
                    self.pick_at_cursor();
                }
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

                let camera = self.camera.camera();
                let mut actions = Vec::new();
                let scene = self
                    .session
                    .as_ref()
                    .and_then(|s| loom_scene::Scene::parse(s.text()).ok());

                let state = PanelState {
                    paths: &self.editable,
                    selected: self.selected,
                    history: self.session.as_ref().map_or(&[][..], |s| s.history()),
                    can_undo: self.session.as_ref().is_some_and(loom_scene::Session::can_undo),
                    can_redo: self.session.as_ref().is_some_and(loom_scene::Session::can_redo),
                    dirty: self.dirty,
                    scene: scene.as_ref(),
                    registry: &self.registry,
                };

                let result = match (self.viewer.as_mut(), self.ui.as_mut(), self.window.as_ref()) {
                    (Some(viewer), Some(ui), Some(window)) => viewer.draw_with_ui(
                        &self.objects,
                        &camera,
                        Some((ui, window)),
                        |root| actions.extend(crate::panels::draw(root, &state)),
                    ),
                    (Some(viewer), _, _) => viewer.draw(&self.objects, &camera),
                    _ => Ok(()),
                };
                if let Err(e) = result {
                    eprintln!("loom: draw failed: {e}");
                    event_loop.exit();
                }
                drop(scene);
                for action in actions {
                    self.act(action);
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
    /// Apply one UI action. Every path funnels through `nudge`/`set_field`,
    /// which funnel through `Session::apply` — one transaction path, one undo
    /// stack, whether the request came from a panel, a key, or the agent.
    fn act(&mut self, action: UiAction) {
        match action {
            UiAction::Select(index) => self.selected = index,
            UiAction::SetField(node, field, value) => self.set_field(&node, &field, value),
            UiAction::Undo => {
                if let Some(s) = self.session.as_mut() {
                    s.undo();
                    self.dirty = true;
                }
            }
            UiAction::Redo => {
                if let Some(s) = self.session.as_mut() {
                    s.redo();
                    self.dirty = true;
                }
            }
            UiAction::Save => {
                if let Some(s) = self.session.as_ref() {
                    match s.save() {
                        Ok(()) => {
                            eprintln!("loom: saved {}", self.scene_path.display());
                            self.dirty = false;
                        }
                        Err(e) => eprintln!("loom: save failed: {e}"),
                    }
                }
            }
        }
    }

    /// Set one field through a transaction.
    fn set_field(&mut self, node: &str, field: &str, value: serde_json::Value) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        // `Transform.pos` is the sugar's desugared name; the op layer writes
        // the node key back out, so the file keeps its canonical form.
        let transaction = if let Some(component_field) = field.strip_prefix("Transform.") {
            let v: Vec<f32> = serde_json::from_value(value.clone()).unwrap_or_default();
            if v.len() != 3 {
                return;
            }
            let axis = [v[0], v[1], v[2]];
            loom_scene::Transaction {
                label: format!("Set {node} transform {component_field}"),
                ops: vec![match component_field {
                    "pos" => loom_scene::SceneOp::SetTransform {
                        node: node.to_owned(), pos: Some(axis), rot_euler: None, scale: None,
                    },
                    "rot_euler" => loom_scene::SceneOp::SetTransform {
                        node: node.to_owned(), pos: None, rot_euler: Some(axis), scale: None,
                    },
                    _ => loom_scene::SceneOp::SetTransform {
                        node: node.to_owned(), pos: None, rot_euler: None, scale: Some(axis),
                    },
                }],
                dry_run: false,
                expect_version: None,
            }
        } else {
            loom_scene::Transaction {
                label: format!("Set {node} {field}"),
                ops: vec![loom_scene::SceneOp::SetField {
                    node: node.to_owned(),
                    field: field.to_owned(),
                    value,
                }],
                dry_run: false,
                expect_version: None,
            }
        };

        match session.apply(transaction) {
            Ok(_) => self.dirty = true,
            // A dragged slider can leave the schema range mid-drag; the
            // rejection is correct and should not spam the log every frame.
            Err(e) if e.error == "would_produce_invalid_scene" => {}
            Err(e) if e.error == "stale_version" => {
                eprintln!("loom: the scene changed on disk — reloading rather than overwriting");
                let _ = session.reload();
            }
            Err(e) => eprintln!("loom: {e}"),
        }
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
        let (w, h) = (w as f32, h as f32);

        // Cursor to a world-space ray through the camera.
        let ndc_x = (self.cursor.0 / w) * 2.0 - 1.0;
        let ndc_y = 1.0 - (self.cursor.1 / h) * 2.0;
        let camera = self.camera.camera();
        let forward = (camera.target - camera.eye).normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(forward);
        let tan = (camera.fov_y_degrees.to_radians() * 0.5).tan();
        let dir = (forward + right * (ndc_x * tan * (w / h)) + up * (ndc_y * tan)).normalize_or_zero();

        let mut best: Option<(f32, &String)> = None;
        for (path, bounds) in &self.picks {
            if let Some(t) = ray_box(camera.eye, dir, bounds)
                && best.is_none_or(|(d, _)| t < d)
            {
                best = Some((t, path));
            }
        }

        if let Some((_, path)) = best
            && let Some(index) = self.editable.iter().position(|p| p == path)
        {
            self.selected = index;
            eprintln!("loom: selected {}", self.editable[self.selected]);
        }
    }

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
    picks: std::collections::BTreeMap<String, loom_scene::place::Bounds>,
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
        picks,
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
    let picks = boxes.clone();

    // Read-only unless asked. Brief §2 puts the editable editor at M12 and a
    // read-only viewer at M5.5, and a viewer that cannot write cannot race the
    // agent's writes at all.
    let session = editable
        .then(|| loom_scene::Session::open(std::path::Path::new(path)))
        .transpose()
        .map_err(|e| format!("{path}: {e}"))?;
    let names: Vec<String> = scene.nodes().iter().map(|n| n.path.clone()).collect();

    run(path, objects, library.into_meshes(), scene_bounds, session, names, picks)
}
