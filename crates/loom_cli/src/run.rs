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
use loom_render::{Camera, Device, Instance, Ui, Viewer, ash, ash_window, egui};

use loom_editor::gizmo::{self, Mode};
use loom_editor::panels::{PanelState, UiAction};
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

/// A unit vector, or forward when there is nothing to normalise.
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length < 1e-6 {
        return [0.0, 0.0, -1.0];
    }
    [v[0] / length, v[1] / length, v[2] / length]
}

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

/// The node a scene's front end opens on.
///
/// **A convention, not a schema.** A scene either has a shot to open on or it
/// does not; nothing else marks a front end, so there is no flag to pass and no
/// component to add. The camera is authored `active = false`, which is what
/// keeps it out of `World::active_camera` and out of `player_character`'s walk
/// — so where it sits in the file cannot matter.
const TITLE_CAM: &str = "Rig/TitleCamera";

/// The front end: the shot it opens on, and the curtain over it.
///
/// **Rendering only.** A camera has never been in the physics hash and this one
/// is not even in the world — it is three numbers read once at load. The
/// curtain is one rectangle in an overlay no headless path constructs. ADR
/// 0045's line is nowhere near either.
struct Front {
    /// Where the shot sits, read once from the scene's title camera.
    shot: Camera,
    /// Seconds into the curtain's own clock.
    ///
    /// **It starts at the top of the fade-*up*, which is what makes the window
    /// open black and lift into the shot** rather than snapping on after half a
    /// second of nothing. One curve, two entrances: the same function that
    /// takes the picture away is the one that brings it in, so there is no
    /// second timeline and no second easing to keep in step.
    clock: f32,
    /// Whether Start has been clicked. Until it has, the menu is up and the
    /// clock is past the end of the curve.
    leaving: bool,
}

/// The context the viewer runs in. A menu would push another.
const FLY: &str = "fly";
/// Driving a character while Play runs. Live only then.
const PLAY: &str = "play";
/// Editing actions, live only when `--edit` was passed.
const EDIT: &str = "edit";
/// What Play prints when it hands a character over.
///
/// One string rather than one per call site, because it is the only place
/// the key list is written down for a player at runtime and it went stale
/// once already: it still said "Esc frees the pointer" after Escape became
/// the pause menu.
const PLAY_KEYS: &str = concat!(
    "WASD to move \u{b7} mouse to look \u{b7} left click to fire \u{b7} ",
    "Space to jump \u{b7} E to interact \u{b7} Esc to pause",
);
/// What one press of Escape means right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    /// Take the menu down and give the game back the pointer.
    Resume,
    /// Freeze the game and put the menu up.
    Pause,
    /// Close the window.
    Close,
}

/// Decide what Escape does, out of the event handler so it can be tested.
///
/// **A free function because nothing headless can press a key.** `App` needs a
/// window, a device and a swapchain, so the only thing about this decision that
/// can be checked without one is the decision itself — and it is the part that
/// was reported broken.
///
/// **What it used to be:** `if captured { hand the pointer back } else { exit }`,
/// with no menu and no pause. So the first Escape while driving released the
/// pointer and *nothing else* — the simulation kept running, and the view fell
/// back to the fly camera, because the frame only uses the character's camera
/// while `self.captured`. That is the "back to the editor type view" in the
/// report.
/// The second Escape then found `captured` false, took the `else`, and closed
/// the window: "it just closes everything". Two presses to lose a session.
///
/// `captured` is in the condition and not just `playing`: a Play session in the
/// editor that the human has clicked out of is playing but not captured, and
/// [`App::set_pause_menu`] recaptures on resume — so opening the menu from an
/// uncaptured state would hand Resume a pointer the human never gave it.
const fn escape_means(menu_open: bool, playing: bool, captured: bool) -> Escape {
    if menu_open {
        Escape::Resume
    } else if playing && captured {
        Escape::Pause
    } else {
        Escape::Close
    }
}

/// Crockford base32 — the ten digits and the letters, minus `I`, `L`, `O`, `U`.
///
/// **The constraint is a voice channel, not a URL.** A room code is read aloud
/// to a friend, so the alphabet drops the glyphs that collide when spoken or
/// squinted at (`I`/`1`, `O`/`0`, `L`/`1`) and drops `U` so a code cannot
/// spell the shorter obscenities by accident.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Symbols in a room code. Twelve of them, five bits each, so **60 bits**.
///
/// **Length is the one number that can never change later** — it is baked into
/// every client that will ever parse a code, and Among Us was forced through
/// exactly that migration once its space filled. So the argument for twelve
/// over ten is laziness rather than paranoia: at 60 bits the code is safe as a
/// *bearer token* (146 years at an unthrottled 10,000 guesses a second against
/// 25,000 live rooms), which deletes rate limiting, a ban list and a separate
/// join password from a transport that does not exist yet. Two characters
/// removes a subsystem.
///
/// Twelve also divides into three groups of four, which ten does not, and 60
/// bits fits a `u64` with four to spare, so the encoder is a shift loop with no
/// padding case.
const CODE_SYMBOLS: usize = 12;

/// Lay `bits` out as base32 symbols, most significant first.
///
/// `symbols` is a parameter because the collision test encodes short codes to
/// make the birthday arithmetic observable at a length where collisions
/// actually happen. Nothing else varies it.
///
/// **The mask is inside the loop.** Applying it once at the call site instead
/// silently drops a symbol of entropy the day someone passes a full `u64`.
fn encode(bits: u64, symbols: usize) -> String {
    (0..symbols)
        .rev()
        .map(|i| char::from(CODE_ALPHABET[((bits >> (i * 5)) & 31) as usize]))
        .collect()
}

/// `XXXXXXXXXXXX` → `XXXX-XXXX-XXXX`. Grouping is what makes it readable aloud.
fn grouped(symbols: &str) -> String {
    format!(
        "{}-{}-{}",
        &symbols[0..4],
        &symbols[4..8],
        &symbols[8..CODE_SYMBOLS]
    )
}

/// The low 60 bits of `bits` as a room code.
fn format_code(bits: u64) -> String {
    grouped(&encode(bits, CODE_SYMBOLS))
}

/// A fresh room code, from the operating system's entropy.
///
/// **This is outside the deterministic core, and that is correct placement
/// rather than a compromise.** A room code names a session, not a simulation:
/// it never enters a `.loom` file, never enters the physics hash, and is never
/// readable by `loom sim --assert` or by rhai. A scene that carried its own
/// room code would render differently in three fresh processes and
/// `cargo xtask repeat` would fail it — correctly.
///
/// `getrandom` rather than `rand`: the syscall is the whole requirement, and
/// `getrandom v0.4.3` is **already in this crate's tree** via `uuid`, so
/// naming it adds no `[[package]]` entry to `Cargo.lock`.
///
/// `None` rather than a panic if the OS refuses. That refusal is close to
/// impossible on Linux, but this runs in `App::new` on every windowed
/// `loom run`, and taking the whole viewer down over a decoration nobody can
/// use yet is the wrong trade at any probability.
///
/// Rerolls an unfortunate code — see [`CODE_DENY`]. Eight attempts because
/// the loop must terminate even if somebody puts a one-character entry in the
/// list; at the measured rejection rate the second attempt is already
/// essentially never reached.
fn generate_code() -> Option<String> {
    (0..8).find_map(|_| {
        let code = format_code(getrandom::u64().ok()?);
        wholesome(&code).then_some(code)
    })
}

/// What the alphabet must not be allowed to spell.
///
/// **Dropping `I`, `L`, `O` and `U` kills most of the English list outright**
/// — anything needing one of those four is unspellable here, which is why
/// this list is as short as it is. What survives is what a vowel-poor
/// alphabet can still manage, and the measured rate is not negligible:
/// 32⁻⁴ per aligned group over three groups is **1 in 18,396** codes carrying
/// one as a whole group, and 1 in 6,132 anywhere in the twelve symbols
/// (measured 1 in 6,431 over four million draws, closed form 1 in 6,132).
///
/// A room code is read aloud and screenshotted. One in six thousand is rare
/// enough never to have shown up in testing and common enough to happen to
/// somebody, which is the worst of both.
const CODE_DENY: &[&str] = &[
    "RAPE", "FAG", "KKK", "TWAT", "WANK", "TARD", "SPAZ", "PAKY", "HEEB", "ARSE", "GASH", "SKAG",
];

/// Whether a code is safe to show somebody.
///
/// Checked on the **ungrouped** symbols, so a word straddling a dash is
/// rejected too. That over-rejects — nobody reads `XXXR-APEX` as a word — but
/// over-rejecting costs one more draw from the OS and under-rejecting costs a
/// screenshot, and the asymmetry is not close.
fn wholesome(code: &str) -> bool {
    let symbols = code.replace('-', "");
    !CODE_DENY.iter().any(|bad| symbols.contains(bad))
}

/// Read a code somebody typed or pasted, or `None` if it cannot be one.
///
/// Nothing calls this yet — there is nothing to join. It ships with the
/// generator because the two are one decision, and because the forgiving half
/// of an alphabet is the half that rots when it is written six weeks later.
///
/// Forgives what a voice channel and a chat window do to a code: any case,
/// separators anywhere (people paste `K-7-M-Q` out of Discord one keypress at
/// a time), and the four excluded letters mapped to what the speaker meant —
/// `I`/`L` → `1`, `O` → `0`, `U` → `V`. Since none of those four can appear in
/// a generated code, mapping them can only ever help.
#[cfg_attr(not(test), expect(dead_code, reason = "no join flag yet — ADR 0065 M4"))]
fn normalise(typed: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_SYMBOLS);
    for c in typed.chars() {
        match c.to_ascii_uppercase() {
            '-' | '_' | ' ' | '\t' => continue,
            'I' | 'L' => out.push('1'),
            'O' => out.push('0'),
            'U' => out.push('V'),
            c if u8::try_from(c).is_ok_and(|b| CODE_ALPHABET.contains(&b)) => out.push(c),
            _ => return None,
        }
        // Rejects nothing the final gate below would not also reject — it
        // is a bound on work, so a hostile ten-megabyte paste stops after
        // thirteen characters rather than being transcribed first.
        if out.len() > CODE_SYMBOLS {
            return None;
        }
    }
    (out.len() == CODE_SYMBOLS).then(|| grouped(&out))
}

/// Wide enough for twelve monospace glyphs and their dashes at 26 points.
///
/// **200 was not, and every code in the game wrapped onto two ragged centred
/// lines.** Bisected through a real `egui::Context` on the widest code the
/// alphabet can spell (`MMMM-WWWW-QQQQ`): it wraps at ≤ 218.484 and fits at
/// ≥ 218.485, at every viewport and every `pixels_per_point`, because egui
/// lays out in points. 240 is that plus a tenth, so a future font tweak has
/// somewhere to go.
///
/// The guard is `the_pause_menu_shows_the_room_code_in_its_top_right`, and it
/// had to be taught to see this: `Galley::text()` returns the *source* string,
/// so an assertion on the drawn text passes just as happily on two fragments.
/// The row count is the assertion.
const CODE_WIDTH: f32 = 240.0;

/// Draw ROOM CODE and the code in the top right of the game's view.
///
/// **Its own `Area`, not a row inside [`crate::hud::pause_menu`].** That menu
/// is a 180-pixel column centred in the viewport, so appending to it would put
/// the code in the middle of the screen. The human asked for the top right.
///
/// Anchored off `available_rect_before_wrap` rather than `Area::anchor`, for
/// the reason the HUD already learned: `anchor` measures from the edges of the
/// whole window, which under `--edit` is on top of the inspector.
///
/// **`taken` is where the HUD just painted, and the panel steps below it.**
/// The top right is not vacant: `proving_ground.loom` anchors its kill counter
/// `top_right` at `offset = [22, 16]`, and the pause menu does not clear
/// `playing`, so it is still on screen while the menu is up — the room code
/// landed straight across it, 101 x 17 pixels of text on text, identically at
/// every resolution because both are fixed pixel offsets from the same corner.
///
/// Dodging beats the two alternatives. Hiding `only_in_play` rows during a
/// pause contradicts the scrim's stated job one screen up (it *dims* the score
/// rather than deleting it), and "scenes must not author `top_right`" would be
/// the seventh unwritten scene-authoring rule this project has accumulated.
/// `hud::draw` was already returning these rects for its own tests and the
/// call site was discarding them, so this costs one binding.
fn room_code_panel(root: &mut egui::Ui, code: &str, taken: &[egui::Rect]) {
    let viewport = root.available_rect_before_wrap();
    let left = viewport.right() - CODE_WIDTH - 24.0;
    // Anything the HUD painted that overlaps this column pushes the panel
    // below it. `max` over all of them rather than the first: two stacked
    // rows in the same corner would otherwise only move it past the higher.
    let top = taken
        .iter()
        .filter(|rect| rect.right() > left && rect.left() < viewport.right() - 24.0)
        .fold(viewport.top() + 24.0, |top, rect| top.max(rect.bottom() + 12.0));
    egui::Area::new(egui::Id::new("loom_room_code"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(left, top))
        .show(root.ctx(), |ui| {
            ui.set_width(CODE_WIDTH);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("ROOM CODE")
                        .size(15.0)
                        .color(egui::Color32::from_gray(185)),
                );
                ui.label(
                    egui::RichText::new(code)
                        .monospace()
                        .size(26.0)
                        .color(egui::Color32::WHITE),
                );
            });
        });
}

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
    /// Decoded textures, so a rebuild does not re-read every PNG from disk.
    /// See [`crate::TextureCache`] — this was most of a gizmo drag's cost.
    texture_cache: crate::TextureCache,
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
    /// The docked panel arrangement, built with the window and saved on exit.
    ///
    /// `None` until `resumed`, and for the same reason as `ui`: there is no
    /// layout without a window to lay out.
    dock: Option<loom_editor::Dock>,
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
    /// A cursor move arrived and the drag has not been applied yet.
    ///
    /// See the comment in the `CursorMoved` arm: the drag is applied once per
    /// frame because applying it per event rebuilt the scene sixteen times a
    /// frame.
    drag_dirty: bool,
    /// Mesh set currently on the GPU, so a moved node costs no re-upload.
    uploaded: u64,
    /// Materials currently on the GPU, keyed separately from the meshes: an
    /// albedo edit moves no vertex, and moving a node changes no colour.
    materials_uploaded: u64,
    /// What the grass on the GPU was placed from — see [`crate::grass_key`].
    /// Empty means "no blades uploaded", which is also a scene with no grass.
    grass_uploaded: String,
    /// What the terrain height grid on the GPU was baked from — see
    /// [`crate::terrain_key`]. Empty means none, which is also a scene with no
    /// water to read it.
    terrain_uploaded: String,
    /// The same grid, kept rather than dropped after the upload.
    ///
    /// The GPU reads it for the water's depth; the CPU reads it to decide
    /// whether the eye is under the surface, and both have to be the same bed
    /// or the shoreline and the underwater view disagree about where the water
    /// stops. Re-baking it per frame is a march down the SDF per sample, which
    /// is not frame work.
    terrain: Option<loom_voxel::heightfield::HeightField>,
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
    /// Scene sounds, open only while Play is running. Silence belongs to the
    /// editor: an ambience loop droning while you drag a gizmo is the audio
    /// equivalent of particles animating before you press Play.
    sound: Option<crate::sound::Sound>,
    /// The game result already reported, so the log line is written once
    /// rather than on every frame after the game ends.
    reported_status: Option<&'static str>,
    /// How many script-fired explosions have already been given to the
    /// particle systems. The runner's list only grows, so the tail past this
    /// is what is new — no event queue, and nothing to miss if a frame is slow.
    detonations_seen: usize,
    /// The same, for splashes. A separate count because the two lists grow
    /// independently and a shared one would replay whichever moved last.
    splashes_seen: usize,
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
    /// Seconds of weather the window has shown, which is what bends the grass.
    ///
    /// **Free-running, and not the simulation clock.** Grass is rendering-only
    /// and outside the determinism hash, so its sway does not have to agree
    /// tick-for-tick with a headless render, and foliage that stands frozen
    /// until you press Play is wrong in an editor — every engine animates it in
    /// the viewport. Advanced from the same frame `dt` the camera uses.
    ///
    /// It is the *simulation's* wind that must stay tick-derived: particles get
    /// theirs from `Plumes`, which advances on steps, not on seconds.
    wind_seconds: f32,
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
    /// Start the simulation as soon as there is a window.
    ///
    /// **This exists so a gate can reach Play.** The per-frame CPU cost that
    /// took `forest.loom` to 9 fps only happens while the simulation advances,
    /// and nothing headless could get there — `--frames` alone runs a paused
    /// editor, which is exactly the case where the defect costs nothing.
    autoplay: bool,
    /// Whether the pause menu is up.
    ///
    /// Its own flag rather than `!captured && play.is_some()`, because those
    /// two are also true of a play session in the editor that the human has
    /// merely clicked out of, and the menu must not appear for that.
    pause_menu: bool,
    /// The front end, while it is up. `None` in the editor, in a scene with
    /// no title camera, and from the moment the curtain finishes lifting on
    /// the game.
    front: Option<Front>,
    /// This session's room code, shown in the pause menu. `None` under
    /// `--edit`, because an authoring session hosts nothing.
    ///
    /// **It identifies a session nobody can join yet.** There is no transport;
    /// ADR 0065 has the plan and the reasons the transport is not this commit.
    /// Generated once here rather than per frame, so the code a player reads
    /// out does not change under them.
    room_code: Option<String>,
    /// How many frames' CPU cost has been measured, and their total.
    ///
    /// **The frame's CPU work, up to the draw call — not the whole frame.**
    /// Past the draw the thread is waiting on a queue and a presentation
    /// engine, and folding that in would make the number mostly a measure of
    /// the GPU and the vsync mode.
    cpu_frames: u32,
    cpu_total_ms: f64,
    cpu_worst_ms: f32,
    /// The draw call itself: the sort, the uploads, the record, the submit and
    /// whatever the presentation engine makes the thread wait for.
    ///
    /// **`cpu` above is not the frame and never claimed to be.** With the GPU
    /// graph at 0.56 ms on `plough_cinematic` and `cpu` at 10.6, the frame was
    /// still 24.8 ms, and there was no instrument anywhere that could say what
    /// the other fourteen were. Splitting the frame in two at the draw call is
    /// the smallest thing that answers it, and `wall` below is the only number
    /// in this project that is the frame rate the human sees.
    draw_total_ms: f64,
    /// Wall time between consecutive redraws — the actual frame rate.
    wall_total_ms: f64,
    wall_worst_ms: f32,
    wall_last: Option<std::time::Instant>,
    /// The cinematic tier's presentation cost, summed: density, march, spray.
    fluid_draw_ms: (f64, f64, f64),
    fluid_draw_frames: u32,
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
        // A read-only viewer or a `--play` session is the one that could host;
        // an `--edit` session is somebody authoring the file.
        let room_code = session.is_none().then(generate_code).flatten();
        Self {
            // The scene's own camera when it has one, the whole scene framed
            // when it does not.
            camera: view
                .camera()
                .map_or_else(|| FlyCamera::framing(view.bounds), FlyCamera::at),
            drag_dirty: false,
            uploaded: view.mesh_key,
            materials_uploaded: view.material_key,
            // Not `grass_key(&view.scene)`: the viewer does not exist yet, so
            // nothing has been uploaded. `resumed` does the first upload.
            grass_uploaded: String::new(),
            terrain_uploaded: String::new(),
            terrain: None,
            selected: view.paths.first().cloned().into_iter().collect(),
            view,
            base,
            voxels: crate::VoxelCache::default(),
            texture_cache: crate::TextureCache::new(),
            session,
            scene_path,
            disk_seen,
            drag: None,
            gesture_epoch: 0,
            play: None,
            mode: Mode::Move,
            handles: Vec::new(),
            play_objects: Vec::new(),
            sound: None,
            reported_status: None,
            detonations_seen: 0,
            splashes_seen: 0,
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
            dock: None,
            gpu: None,
            #[allow(clippy::disallowed_methods)]
            last_frame: std::time::Instant::now(),
            wind_seconds: 0.0,
            fps: 0.0,
            frames_left: None,
            autoplay: false,
            front: None,
            pause_menu: false,
            room_code,
            cpu_frames: 0,
            cpu_total_ms: 0.0,
            cpu_worst_ms: 0.0,
            draw_total_ms: 0.0,
            wall_total_ms: 0.0,
            wall_worst_ms: 0.0,
            wall_last: None,
            fluid_draw_ms: (0.0, 0.0, 0.0),
            fluid_draw_frames: 0,
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
    ) -> Vec<loom_editor::panels::AgentMark> {
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
            marks.push(loom_editor::panels::AgentMark {
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
        let view = match SceneView::build_cached(
            text,
            &self.base,
            &mut self.voxels,
            &mut self.texture_cache,
        ) {
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

        // **The colours, separately from the geometry.** Editing a `Material`
        // moves no vertex, so `mesh_key` does not budge and this used not to
        // run at all — the file changed, the inspector showed the new value,
        // and the viewport went on drawing the old one until the process was
        // restarted. Keyed independently because the reverse is just as
        // common: moving a node must not re-upload every texture in the scene.
        if view.material_key != self.materials_uploaded
            && let Some(viewer) = self.viewer.as_mut()
        {
            match viewer.set_materials(view.textures(), view.material_table()) {
                Ok(()) => self.materials_uploaded = view.material_key,
                Err(e) => crate::log::error(format!("could not upload the new materials: {e}")),
            }
        }

        // A node the agent deleted cannot stay selected.
        self.selected.retain(|p| view.paths.contains(p));
        if self.selected.is_empty() {
            self.selected.extend(view.paths.first().cloned());
        }
        self.view = view;
        // Grass is placed from the scene the same way the meshes are, so a
        // reload has to re-place it or the window keeps showing the old field.
        self.upload_grass();
        self.upload_terrain();
        // The scene changed, so the emitters may have. Dropped rather than
        // patched: a reload is rare and rebuilding is a warm-up, not a frame
        // cost.
        self.plumes = None;
    }

    /// Place this scene's grass and hand it to the GPU, if it changed.
    ///
    /// Placement is a pure function of position, so this is not per-frame work
    /// — the blades are uploaded once and the vertex shader expands and bends
    /// them every frame. [`crate::grass_key`] is what keeps it that way while
    /// `show` runs on every frame of a gizmo drag.
    fn upload_grass(&mut self) {
        let key = crate::grass_key(&self.view.scene);
        if key == self.grass_uploaded {
            return;
        }
        let blades = crate::grass_blades(&self.view.scene);
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        // The same silent-truncation trap the offscreen path has, except here
        // it lands in the human's console rather than on stderr, which is where
        // they are actually looking.
        let capacity = viewer.grass_capacity();
        if blades.len() > capacity && capacity > 0 {
            crate::log::error(format!(
                "the grass field needs {} blades and the buffer holds {capacity}; {} were \
                 dropped in generation order, so expect a hard edge across the field. \
                 Reduce density or half_extent.",
                blades.len(),
                blades.len() - capacity
            ));
        }
        match viewer.set_grass(&blades) {
            Ok(()) => self.grass_uploaded = key,
            Err(e) => crate::log::error(format!("could not upload the grass: {e}")),
        }
    }

    /// Bake this scene's terrain height grid and hand it to the GPU, if it
    /// changed.
    ///
    /// **This is where a carved lake bed reaches the water.** The key is the
    /// volume's op list, so the transaction that blows a crater rebakes and the
    /// shoreline moves with it; a gizmo drag that touches nothing else does
    /// not, because the bake is a march down the SDF per sample and that is not
    /// frame work.
    fn upload_terrain(&mut self) {
        let key = crate::terrain_key(&self.view.scene);
        if key == self.terrain_uploaded {
            return;
        }
        let field = crate::scene_terrain_field(&self.view.scene);
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        let result = match field.as_ref() {
            Some(f) => viewer.set_terrain(&f.height, f.origin, f.spacing, f.side),
            None => viewer.set_terrain(&[], [0.0; 2], 1.0, 0),
        };
        // **The current, on the same trigger and for the same reason** — it is
        // routed off the bed that was just baked, so carving the bank rebakes
        // both. Uploaded here rather than per frame because it is a bake, not
        // state: the ripple grid beside it is the one that moves every tick.
        let world = self.view.world();
        let flow = field.as_ref().and_then(|f| {
            crate::weather::water_of(world, &crate::weather::wind_of_world(world))
                .and_then(|body| crate::river_flow(f, &body))
        });
        let flow_result = match flow.as_ref() {
            Some(g) => viewer.set_flow(g.velocities(), g.origin, g.spacing, g.side),
            None => viewer.set_flow(&[], [0.0; 2], 1.0, 0),
        };
        if let Err(e) = flow_result {
            crate::log::error(format!("could not upload the river current: {e}"));
        }
        // **And the world raindrops collide with, on exactly the same trigger.**
        // Carving the roof open in the editor lets rain through on the next
        // frame because the field was re-baked, not because anything told the
        // rain about it — which is Phase 4's sharpest exit criterion and now
        // applies to a mesh gantry as well as to a voxel roof.
        let rain_field = crate::rain_collision_field(&self.view.scene, self.view.world());
        if let Some(f) = rain_field.as_ref()
            && let Err(e) = viewer.set_rain_field(&f.sdf, f.dims, f.origin, f.spacing)
        {
            crate::log::error(format!("could not upload the rain collision field: {e}"));
        }
        match result {
            Ok(()) => {
                self.terrain_uploaded = key;
                self.terrain = field;
                // Carving the roof open in the editor lets rain in on the next
                // frame with no reload, and the two bakes above are the whole
                // reason: the height grid is where a drop falls to outside the
                // collision field, and the collision field is what a drop
                // actually tests against. Nothing per-frame marches the SDF.
            }
            Err(e) => crate::log::error(format!("could not upload the terrain heights: {e}")),
        }
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

        let mut attributes = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1440, 900));
        // `LOOM_WINDOW_AT=x,y` puts the window on a chosen monitor — the
        // windowed half of `cargo xtask validate` opens five of these, and on a
        // multi-head desk they land wherever the WM feels like. Physical
        // pixels, and a hint: X11 honours it, a compositor may not.
        if let Some((x, y)) = std::env::var("LOOM_WINDOW_AT").ok().and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some((x.trim().parse::<i32>().ok()?, y.trim().parse::<i32>().ok()?))
        }) {
            attributes = attributes.with_position(winit::dpi::PhysicalPosition::new(x, y));
        }
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
                    Ok(ui) => {
                        // The palette, before the first frame is laid out —
                        // `Style` affects sizes as well as colours, so
                        // applying it later would make frame one a different
                        // shape from every frame after it.
                        loom_editor::apply_theme(ui.context(), &loom_editor::tokens(false));
                        self.ui = Some(ui);
                        // **`--frames` never reads or writes the saved
                        // layout.** The windowed half of `cargo xtask validate`
                        // opens a window per scene, and a gate whose viewport
                        // depends on where the human last dragged a splitter is
                        // not a gate. It would also overwrite that layout on
                        // the way out.
                        //
                        // Logical points, because `egui_dock` splits by
                        // fraction and `BOTTOM_HEIGHT` is authored in points.
                        let height = window
                            .inner_size()
                            .to_logical::<f32>(window.scale_factor())
                            .height;
                        // **`--play` without `--edit` is the game, not a tool
                        // around the game.** The panels used to be
                        // unconditional, so the one command a demo's own header
                        // documents opened a Hierarchy, an Inspector, a Console
                        // and a toolbar with Delete on it, and left the game
                        // 876x576 of a 1440x900 window — 39%. A stranger's
                        // first frame should be the scene.
                        //
                        // `--edit` still gets everything, and so does a plain
                        // `loom run` with no flags, which is the read-only
                        // viewer M5.5 asked for. `cargo xtask validate` passes
                        // `--edit` on both its windowed rows, so the gate is
                        // untouched.
                        if self.session.is_some() || !self.autoplay {
                            self.dock =
                                Some(loom_editor::Dock::new(self.frames_left.is_none(), height));
                        }
                    }
                    Err(e) => crate::log::error(format!("no editor UI ({e}); continuing bare")),
                }
                self.gpu = Some((instance, device));
                // **The scene is a rectangle rather than the whole window.**
                // Only when there is a UI to leave room for it — bare mode and
                // `--frames` still fill the window, and so does every headless
                // path, which is what keeps `loom render` and `loom run` in
                // agreement.
                //
                // On by default now that input goes through `App::projection`:
                // every cursor position is mapped into the rectangle the scene
                // was actually drawn in, so picking, the gizmo handles and the
                // agent overlay all land where they look. It was behind
                // `LOOM_DOCK_VIEWPORT=1` for exactly one stage, while that was
                // not true.
                let mut viewer = viewer;
                viewer.set_dock_viewport(self.ui.is_some());
                self.viewer = Some(viewer);
                self.window = Some(window);
                // The meshes went in through `Viewer::new`; grass has no such
                // constructor argument, so the first field is placed here.
                self.upload_grass();
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
                // **The drag is applied once per frame, not once per event.**
                // A gizmo drag rebuilds the whole `SceneView` — mesh library,
                // material library, scatter, bounds — and a mouse reporting at
                // 1000 Hz delivers about sixteen `CursorMoved` events per
                // 60 Hz frame. That was sixteen full rebuilds per frame, and
                // on `materials.loom` a rebuild is 4.2 ms in release: about
                // 67 ms of work for one frame of dragging, which is the stall
                // that got reported.
                //
                // The inspector never showed it because egui resolves a
                // `DragValue` once per frame by construction, so its edit rate
                // was already one per frame. This makes the gizmo match.
                //
                // Coalescing the *transaction* was already handled — the
                // gesture key means a drag is one undo step — but that
                // collapses the undo history, not the work.
                self.drag_dirty = true;
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
                // The frame's one application of whatever the mouse did since
                // the last one. Absolute from the drag's start rather than
                // accumulated, so collapsing events loses nothing.
                if std::mem::take(&mut self.drag_dirty) {
                    self.drag_gizmo();
                }
                // See the note in `App::new` — presentation, not simulation.
                #[allow(clippy::disallowed_methods)]
                let now = std::time::Instant::now();
                let dt = now.duration_since(self.last_frame).as_secs_f32();
                self.last_frame = now;
                if dt > 0.0 {
                    // Exponential smoothing: readable, and one line.
                    self.fps = self.fps.mul_add(0.9, (1.0 / dt) * 0.1);
                }
                // Clamped for the same reason the camera step is: a stall must
                // not jump the wind forward and snap every blade.
                self.wind_seconds += dt.min(0.1);
                // The curtain runs on the same wall clock as the wind, and
                // is clamped for the same reason: a stall must stretch the
                // fade rather than skip it, so a hitch never jump-cuts.
                //
                // Read out before acting on it — `start_play` takes `&mut
                // self`, and the borrow checker is right that these are the
                // same `self`.
                let leaving = self.front.as_mut().and_then(|front| {
                    front.clock += dt.min(0.1);
                    front.leaving.then_some(front.clock)
                });
                if let Some(clock) = leaving {
                    // Under the black, where the hitch of building a world is
                    // free. This is why the hold is a floor and not a ceiling.
                    if clock >= crate::hud::FADE_OUT && self.play.is_none() {
                        self.start_play();
                    }
                    if clock >= crate::hud::TRANSITION {
                        self.front = None;
                    }
                }
                // Once, and only when there is something to play into.
                if self.autoplay && self.play.is_none() && self.viewer.is_some() {
                    self.autoplay = false;
                    self.start_play();
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
                // **Not while the curtain is moving.** Between Start and the
                // game there is no `Play` and no captured pointer, which
                // `escape_means` reads as `Close` — so a reflexive Escape
                // after clicking Start would kill the window mid-fade. Guarded
                // on the branch rather than by returning early: a `return`
                // here would skip `window.request_redraw()` at the bottom of
                // this arm and freeze the picture at whatever alpha it had.
                let transitioning = self.front.as_ref().is_some_and(|f| f.leaving);
                if !transitioning && self.input.is_active(&self.bindings, FLY, "quit") {
                    match escape_means(self.pause_menu, self.play.is_some(), self.captured) {
                        Escape::Resume => self.set_pause_menu(false),
                        Escape::Pause => self.set_pause_menu(true),
                        Escape::Close => {
                            event_loop.exit();
                            return;
                        }
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
                self.report_game_result();
                if stepped > 0 {
                    self.update_sound();
                }

                // While driving a character the view is the scene's camera,
                // which the character carries. The fly camera is still there
                // and still where it was — Escape hands the pointer back and
                // Stop returns to it.
                let camera = self
                    .play
                    .as_ref()
                    // `|| self.pause_menu`: the menu releases the pointer, and
                    // without this the frame behind it would be the fly camera
                    // — which is exactly the "back to the editor" the menu
                    // exists to stop.
                    .filter(|_| self.captured || self.pause_menu)
                    .and_then(crate::play::Play::camera)
                    .map_or_else(
                        // The title's shot until Play has a camera of its own,
                        // which it gets halfway through the curtain — so the
                        // swap happens under full black and costs nothing. No
                        // blend: the player spawns on a railed deck and every
                        // path from open water to his eye goes through the rig.
                        || match &self.front {
                            Some(front) => front.shot,
                            None => self.camera.camera(),
                        },
                        |view| Camera {
                            eye: Vec3::from_array(view.eye),
                            target: Vec3::from_array(view.target),
                            fov_y_degrees: view.fov_y_degrees,
                        },
                    );
                let projection = self.projection_for(&camera);

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
                // The scene'''s own weather, so a plume in the viewer bends the
                // same way it does in a headless render.
                let wind = crate::weather::wind_of(&self.view.scene);
                let plumes = self
                    .plumes
                    .get_or_insert_with(|| {
                        // Drips need a collision world to find their floor —
                        // ADR 0054 — and only a play session is holding one.
                        // A scene merely being viewed shows no drips rather
                        // than drips falling to a guessed floor.
                        let physics = self.play.as_ref().map(crate::play::Play::physics);
                        crate::particles::Plumes::new(world, wind, physics)
                    });
                plumes.advance(stepped);
                let particles: &[loom_render::ParticleInstance] = plumes.instances();

                // **The cinematic tier, in the window at last** — ADR 0057,
                // defect 5 of the water rebuild review. Play has been stepping
                // the solver here since slice 5 and the window drew none of it:
                // the surface march and the spray readback existed on the
                // headless path only, which is the third time this project has
                // shipped a water effect wired into one path (`set_ripples`,
                // ADR 0046 §7, and the splash crown are the first two).
                //
                // **Per frame drawn, not per tick**, and that is the only thing
                // about it that differs from headless. The readback that ADR
                // 0053 §3 requires to be synchronous and inside the fixed step
                // is `Sim::float_cinematic`'s, which the tick above already
                // ran; marching the density it left is presentation and belongs
                // where the picture is made. A tick with no frame marches
                // nothing.
                //
                // Not gated on `stepped`: the human orbiting a paused scene
                // still has to see the water. The march is idempotent — it
                // reads the solver's density and changes nothing.
                let (fluid_surface, fluid_spray, fluid_cost) = self
                    .play
                    .as_mut()
                    .map(crate::play::Play::fluid_draw)
                    .unwrap_or_default();
                // Summed here and reported beside `cpu ms/frame`, because the
                // window is the only place this is paid **per frame drawn**
                // rather than once — see `Sim::fluid_draw`.
                if !fluid_surface.is_empty() {
                    self.fluid_draw_ms.0 += fluid_cost.density_ms;
                    self.fluid_draw_ms.1 += fluid_cost.march_ms;
                    self.fluid_draw_ms.2 += fluid_cost.spray_ms;
                    self.fluid_draw_frames += 1;
                }

                // Resolved against the play world when one is running, so a
                // HUD element parented to something that moved reads the same
                // world the rules judged.
                let overlay = match self.play.as_ref() {
                    Some(play) => crate::hud::elements(&play.world, play.state(), true, false),
                    None => crate::hud::elements(
                        self.view.world(),
                        &loom_script::GameState::default(),
                        false,
                        self.front.is_some(),
                    ),
                };

                let mut actions = Vec::new();
                // Snapshotted once per frame rather than read inside the
                // panel: the panels crate cannot reach the global store, and
                // one snapshot per frame is also one lock rather than one per
                // row.
                let console = crate::log::entries();
                // From the session's text, which is the *unresolved* file —
                // the resolved `SceneView` has no overrides left to report.
                // Empty in read-only mode, where nothing is revertable anyway.
                let overrides = self
                    .session
                    .as_ref()
                    .map(|s| crate::override_map(s.text()))
                    .unwrap_or_default();
                let state = PanelState {
                    agent_marks: &marks,
                    console: &console,
                    overrides: &overrides,
                    tick_seconds: crate::play::TICK_SECONDS,
                    // Spelled out rather than passed as `&self.view`: the
                    // panels live in `loom_editor`, which cannot see
                    // `SceneView` — see `PanelState`'s doc comment.
                    scene: &self.view.scene,
                    paths: &self.view.paths,
                    picks: &self.view.picks,
                    assets: &self.view.assets,
                    object_count: self.view.objects.len(),
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

                // The scene's sky, from whichever world is current: play mode
                // holds its own, and an environment edited during play should
                // show up like any other edit.
                // **The scene's own wind, at a clock that advances.** This was
                // `environment_of`, which passes `Wind::default()` and a time of
                // zero — so the window bent its grass with the wrong wind and
                // then froze it there. The authored `Wind` reached the particles
                // and never reached the blades.
                let world = match self.play.as_ref() {
                    Some(play) => &play.world,
                    None => self.view.world(),
                };
                let wind = crate::weather::wind_of(&self.view.scene);
                let mut environment =
                    crate::environment_with_wind(world, &wind, self.wind_seconds);
                // Whether the eye is under the water, from the same query that
                // muffles the sound (W7). The fly camera and a swimming
                // character both go through here, so the window's view and the
                // scripts' `is_submerged` cannot disagree.
                crate::submerge_eye(
                    &mut environment,
                    world,
                    &wind,
                    self.terrain.as_ref(),
                    camera.eye,
                    self.wind_seconds,
                );
                // And the rain, through the same call the headless path
                // makes. The window gets the real version rather than a
                // simplified one — three defects this phase were exactly that,
                // with no gate able to see the difference. **Walking the fly
                // camera under the shelter no longer stops the rain outside
                // it**; the per-drop cull in the shader does that, per drop.
                let drops = crate::rain_at_eye(
                    &mut environment,
                    crate::weather::rain_of(&self.view.scene).as_ref(),
                    &wind,
                    camera.eye,
                    self.wind_seconds,
                );
                // Where the rain lands is the GPU's answer now: a splash is a
                // collision `rain_sim.slang` resolved against the baked world,
                // appended to a ring and drawn indirectly (ADR 0015). What does
                // still arrive on the CPU is the cinematic tier's spray, read
                // back inside the fixed step and appended here so it goes
                // through the one particle renderer like everything else
                // (ADR 0047's rule) — the same list the headless path builds.
                let crowns: Vec<loom_render::ParticleInstance> = fluid_spray;
                let combined;
                let particles: &[loom_render::ParticleInstance] = if crowns.is_empty() {
                    particles
                } else {
                    combined = [particles, &crowns].concat();
                    &combined
                };
                if let Some(viewer) = self.viewer.as_mut() {
                    viewer.environment = environment;
                    viewer.set_rain(drops);
                    // **The drop simulation's clock, in ticks.** The same
                    // mapping the headless path uses — `--sim N` is N/60
                    // seconds — so a window and a `loom render --sim N` of the
                    // same scene ask the simulation for the same instant.
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let tick = (self.wind_seconds * 60.0).round().max(0.0) as u64;
                    viewer.set_rain_tick(tick);
                    // **The GPU particle pool, on that same clock.** The window
                    // and `loom render --sim N` therefore ask the pool for the
                    // same instant — which is the whole reason the viewer got
                    // this wiring rather than being left on the CPU path. A
                    // filter measured somewhere the filter is not is the defect
                    // that cost this project a night on MSAA.
                    viewer.set_gpu_emitter(crate::particles::gpu_emitter(world), tick);
                    // **And the wavelet events Play has stepped to** — ADR
                    // 0056. Not on the tick clock above: this is the CPU's own
                    // stepped state, so it is whatever the simulation is
                    // holding right now, and there is nothing for the window to
                    // re-derive. In edit mode there is no simulation and the
                    // call is skipped, which leaves the plain Gerstner surface.
                    if let Some(pool) = self.play.as_ref().map(crate::play::Play::wavelets)
                        && let Err(e) = viewer.set_wavelets(&crate::wavelet_upload(pool))
                    {
                        crate::log::warn(format!("wavelets: {e}"));
                    }
                    // And the foam field, on exactly the same rule — the other
                    // half of the CPU's stepped water state (ADR 0055). Wired
                    // here in the same commit as the headless path, because a
                    // water effect wired on one path only is the defect ADR
                    // 0046 §7 records and nothing in the gate can photograph a
                    // window.
                    if let Some(field) = self.play.as_ref().and_then(crate::play::Play::foam)
                        && let Err(e) = viewer.set_foam(
                            field.coverage(),
                            field.origin(),
                            field.cell(),
                            field.side(),
                        )
                    {
                        crate::log::warn(format!("foam: {e}"));
                    }
                    // **And the cinematic free surface**, marched above. Empty
                    // outside the tier and in edit mode, where there is no
                    // simulation and so no solver — the same rule the two
                    // stepped fields above it follow.
                    if let Err(e) = viewer.set_fluid_surface(&fluid_surface) {
                        crate::log::warn(format!("cinematic surface: {e}"));
                    }
                }

                // Everything above is the frame's CPU work: input, the
                // simulation step, re-deriving draw calls, the weather, the
                // particles and the panels. Sampled here rather than at the end
                // of the handler for the reason `cpu_frames` gives.
                #[allow(clippy::disallowed_methods)]
                let cpu_ms = std::time::Instant::now().duration_since(now).as_secs_f64() * 1000.0;
                self.cpu_frames += 1;
                self.cpu_total_ms += cpu_ms;
                #[allow(clippy::cast_possible_truncation)]
                {
                    self.cpu_worst_ms = self.cpu_worst_ms.max(cpu_ms as f32);
                }

                // Bound out of `self` before the match so the borrow checker
                // sees three disjoint fields rather than one `&mut self`.
                // Instrumentation only — never-do #8 is about the simulation,
                // and nothing below is read by one.
                #[allow(clippy::disallowed_methods)]
                let draw_started = std::time::Instant::now();
                let mut dock = self.dock.as_mut();
                // Read out of `self` before the borrow, like `dock`, and
                // answered after the draw — the build closure is `FnMut` and
                // may run more than once, so the click is recorded rather than
                // acted on inside it.
                let menu_open = self.pause_menu;
                // The front end's two halves, read out like `menu_open` is:
                // whether its menu is up, and how black the screen is.
                let title_up = self.front.as_ref().is_some_and(|f| !f.leaving);
                let curtain = self.front.as_ref().map(|f| crate::hud::curtain(f.clock));
                let room_code = self.room_code.as_deref();
                let mut pause_choice = None;
                let mut title_choice = None;
                let result = match (self.viewer.as_mut(), self.ui.as_mut(), self.window.as_ref()) {
                    (Some(viewer), Some(ui), Some(window)) => viewer.draw_with_ui(
                        drawn,
                        particles,
                        &camera,
                        Some((ui, window)),
                        |root| {
                            // Panels first. The overlay is anchored to
                            // whatever they leave over, and before they are
                            // added that is the entire window — which is how
                            // the score ended up on top of the hierarchy.
                            // The dock carves the viewport tab back out of the
                            // root, so `available_rect_before_wrap` — which is
                            // what `hud::draw` anchors to — is that tab.
                            // Reborrowed rather than moved: the build closure
                            // is `FnMut`, so it may run more than once.
                            if let Some(dock) = dock.as_deref_mut() {
                                actions.extend(dock.draw(root, &state));
                            }
                            // **The title's scrim goes on before the HUD,
                            // and the pause menu's after.** They dim opposite
                            // things: a pause menu dims the game *and* its
                            // score, a title screen dims the sea the game's
                            // own name is written across.
                            if title_up {
                                crate::hud::title_scrim(root);
                            }
                            // The rects are the second half of this return
                            // value and were thrown away until the room code
                            // needed somewhere to stand — see
                            // [`room_code_panel`].
                            let (_, painted) = crate::hud::draw(root, &overlay);
                            if title_up {
                                title_choice = crate::hud::title_menu(root);
                            } else if menu_open {
                                pause_choice = crate::hud::pause_menu(root);
                                if let Some(code) = room_code {
                                    room_code_panel(root, code, &painted);
                                }
                            }
                            // Last of all and in a layer of its own, so it
                            // covers the menu it is dismissing and the panels
                            // beside it.
                            if let Some(alpha) = curtain {
                                crate::hud::fade(root, alpha);
                            }
                        },
                    ),
                    (Some(viewer), _, _) => viewer.draw(drawn, &camera),
                    _ => Ok(()),
                };
                self.draw_total_ms += draw_started.elapsed().as_secs_f64() * 1000.0;
                #[allow(clippy::disallowed_methods)]
                let ended = std::time::Instant::now();
                if let Some(previous) = self.wall_last.replace(ended) {
                    let wall = ended.duration_since(previous).as_secs_f64() * 1000.0;
                    self.wall_total_ms += wall;
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        self.wall_worst_ms = self.wall_worst_ms.max(wall as f32);
                    }
                }
                if let Err(e) = result {
                    eprintln!("loom: draw failed: {e}");
                    event_loop.exit();
                }
                match title_choice {
                    Some(crate::hud::TitleChoice::Start) => {
                        if let Some(front) = self.front.as_mut() {
                            front.clock = 0.0;
                            front.leaving = true;
                        }
                    }
                    Some(crate::hud::TitleChoice::Quit) => {
                        self.shutdown(event_loop);
                        return;
                    }
                    None => {}
                }
                match pause_choice {
                    Some(crate::hud::PauseChoice::Resume) => self.set_pause_menu(false),
                    Some(crate::hud::PauseChoice::Quit) => {
                        self.shutdown(event_loop);
                        return;
                    }
                    None => {}
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
                        self.report_cpu();
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

    /// The layout is written once, on the way out.
    ///
    /// Not per frame and not on every splitter drag: a dock rearrangement is
    /// worth a file at the end of the session, and writing one on each of the
    /// sixty frames a drag spans is a filesystem write per frame for a
    /// convenience. `Dock::save` is a no-op when persistence is off.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(dock) = self.dock.as_ref() {
            dock.save();
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
            UiAction::Splice(node, field, index, remove, insert) => {
                // **Not coalesced into a gesture.** A drag on a slider is one
                // continuous act and undoes as one; adding and removing entries
                // are separate decisions and each earns its own undo step.
                self.transact(
                    format!("Splice {node} {field}"),
                    vec![loom_scene::SceneOp::SpliceArray {
                        node,
                        field,
                        index,
                        remove,
                        insert,
                    }],
                );
            }
            UiAction::RevertOverride(node, field) => {
                let keys = if field.is_empty() { Vec::new() } else { vec![field.clone()] };
                let what = if field.is_empty() { "all overrides".to_owned() } else { field };
                self.transact(
                    format!("Revert {node} {what}"),
                    vec![loom_scene::SceneOp::RevertOverrides { node, keys }],
                );
            }
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
        self.sound = self
            .play
            .as_ref()
            .and_then(|p| crate::sound::Sound::start(&p.world, &self.base, &self.view.scene));
        // First person only when the scene actually has the rig for it: a
        // character to move and a camera to see through. Without both, Play
        // stays a spectator view and the fly camera keeps working.
        if drivable {
            self.capture_pointer(true);
            crate::log::info(PLAY_KEYS);
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

    /// Open or close the pause menu.
    ///
    /// Three things move together and all three are the point: the pointer
    /// comes back so the menu can be clicked, the simulation stops so nothing
    /// eats you while you read it, and `pause_menu` keeps the camera on the
    /// game rather than letting it fall back to the fly camera.
    ///
    /// Resume unpauses unconditionally, so a session paused from the editor
    /// toolbar and then Escaped resumes on Resume. That is what the word says.
    fn set_pause_menu(&mut self, open: bool) {
        self.pause_menu = open;
        self.capture_pointer(!open);
        if let Some(play) = self.play.as_mut() {
            play.paused = open;
        }
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
            interact: self.input.is_active(&self.bindings, PLAY, "interact"),
            bag: self.input.is_active(&self.bindings, PLAY, "bag"),
        });
    }

    /// Stop, and go back to the authored scene exactly as it was.
    fn stop_play(&mut self) {
        self.pause_menu = false;
        self.capture_pointer(false);
        // Stop discards the simulated world, so the explosions in it are gone
        // too. Leaving the counter high would make the first shot of the next
        // run look like one already seen, and it would never be drawn.
        self.detonations_seen = 0;
        self.splashes_seen = 0;
        self.reported_status = None;
        self.plumes = None;
        // Dropping the device closes the stream, so Stop is silent rather
        // than leaving a loop running over an editor nobody is playing.
        if let Some(sound) = self.sound.take() {
            sound.silence();
        }
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

    /// Re-measure what every sound sounds like from where the listener is.
    ///
    /// The listener is the camera, not the character: you hear from where you
    /// look, and in third person those differ.
    fn update_sound(&mut self) {
        let (Some(sound), Some(play)) = (self.sound.as_mut(), self.play.as_ref()) else {
            return;
        };
        let Some(view) = play.camera() else {
            return;
        };
        let forward = normalize([
            view.target[0] - view.eye[0],
            view.target[1] - view.eye[1],
            view.target[2] - view.eye[2],
        ]);
        // Right-handed, world up: forward cross up.
        let right = normalize([forward[2], 0.0, -forward[0]]);
        // The ears' own state, from the simulation that owns the surface —
        // the same water body, clock and bed a crate floats on.
        let submerged = play.submerged_at(view.eye);
        sound.update(&play.world, play.physics(), view.eye, right, forward, submerged);
    }

    /// Say how the game ended, once.
    ///
    /// The window has no HUD, so the console is where a result goes. Without
    /// it a game that ends simply stops moving, which is indistinguishable
    /// from a simulation that broke.
    fn report_game_result(&mut self) {
        let Some(play) = self.play.as_ref() else {
            return;
        };
        let status = play.state().status();
        if !status.is_over() || self.reported_status == Some(status.as_str()) {
            return;
        }
        self.reported_status = Some(status.as_str());
        let message = play.state().message().to_owned();
        let happened: Vec<String> = play
            .events()
            .counts()
            .into_iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect();
        let numbers: Vec<String> = play
            .state()
            .numbers()
            .iter()
            .map(|(name, value)| format!("{name} {value:.0}"))
            .collect();
        crate::log::info(format!(
            "{} — {message}{}{}",
            status.as_str().to_uppercase(),
            if numbers.is_empty() { "" } else { " · " },
            numbers.join(", ")
        ));
        if !happened.is_empty() {
            crate::log::info(format!("events — {}", happened.join(", ")));
        }
    }

    /// Give the particle systems any explosion a script has just set off, and
    /// any splash the water has just raised.
    fn spawn_new_detonations(&mut self) {
        let Some(play) = self.play.as_ref() else {
            return;
        };
        let fired = play.fired();
        let splashed = play.splashed();
        if fired.len() <= self.detonations_seen && splashed.len() <= self.splashes_seen {
            return;
        }
        let fresh: Vec<(u64, [f32; 3])> = fired[self.detonations_seen.min(fired.len())..]
            .iter()
            .map(|(tick, at)| (*tick, *at))
            .collect();
        let wet: Vec<crate::play::Splash> =
            splashed[self.splashes_seen.min(splashed.len())..].to_vec();
        self.detonations_seen = fired.len();
        self.splashes_seen = splashed.len();

        // Against the play world, because that is where the dormant prefab is
        // — the same scene, but the one the simulation is holding.
        let world = &play.world;
        if let Some(plumes) = self.plumes.as_mut() {
            for (tick, at) in fresh {
                plumes.detonate(world, at, tick);
            }
            for splash in wet {
                plumes.splash(world, splash);
            }
        }
    }

    /// What the frame cost on the CPU, for a bounded run.
    ///
    /// One line, parseable, printed when `--frames` runs out. A golden image
    /// renders a single frame and so cannot tell placing once from placing
    /// sixty times a second; this is the number that can.
    fn report_cpu(&self) {
        if self.cpu_frames == 0 {
            return;
        }
        let mean = self.cpu_total_ms / f64::from(self.cpu_frames);
        let draw = self.draw_total_ms / f64::from(self.cpu_frames);
        crate::log::info(format!(
            "cpu {mean:.3} ms/frame mean, {:.3} ms worst over {} frames",
            self.cpu_worst_ms, self.cpu_frames
        ));
        // The frame the human sees, and the split that says which half to fix.
        if self.wall_total_ms > 0.0 && self.cpu_frames > 1 {
            let wall = self.wall_total_ms / f64::from(self.cpu_frames - 1);
            crate::log::info(format!(
                "frame {wall:.3} ms mean ({:.1} fps), {:.3} ms worst — cpu {mean:.3}, \
                 draw {draw:.3}",
                1000.0 / wall,
                self.wall_worst_ms,
            ));
        }
        // **Per frame drawn, and labelled so**, which is the distinction the
        // line below it does not make: `ms/tick` is a per-tick number and a
        // frame runs several ticks. Quoting one as the other is what put
        // cinematic water inside a 60 Hz budget on paper.
        if self.fluid_draw_frames > 0 {
            let n = f64::from(self.fluid_draw_frames);
            crate::log::info(format!(
                "cinematic surface: density {:.2} ms, march {:.2} ms, spray {:.2} ms                  per frame drawn, over {} frames",
                self.fluid_draw_ms.0 / n,
                self.fluid_draw_ms.1 / n,
                self.fluid_draw_ms.2 / n,
                self.fluid_draw_frames
            ));
        }
        // **And what the cinematic tier's device round trip cost**, in the same
        // words the headless path prints — ADR 0053 §4 asks for it to be
        // measured and reported rather than assumed tolerable, and the window
        // is where it is worst: every tick's fence wait now queues behind a
        // frame the same GPU is drawing. Silent above is a scene outside the
        // tier, which is every scene but three.
        if let Some((total, fence, ticks)) = self.play.as_ref().map(crate::play::Play::fluid_cost)
            && ticks > 0
        {
            #[allow(clippy::cast_precision_loss)]
            let n = ticks as f64;
            crate::log::info(format!(
                "cinematic water: {:.2} ms/tick, {:.2} ms of it the device round trip, over \
                 {ticks} ticks",
                total / n,
                fence / n
            ));
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
                prefab: None,
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
                prefab: None,
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
    /// The projection the last frame actually rendered with.
    ///
    /// **One place builds this, and every consumer goes through it.** The
    /// scene may be drawn into a sub-rectangle of the window, so a projection
    /// built from the window's size — which is what every one of these call
    /// sites used to do — is both the wrong aspect ratio and the wrong origin.
    /// Wrong aspect puts the gizmo slightly off the object; wrong origin makes
    /// a click select whatever is that far away instead.
    fn projection(&self) -> gizmo::View {
        self.projection_for(&self.camera.camera())
    }

    /// As [`Self::projection`], for a camera that is not the fly camera —
    /// a scene-authored `Camera` while playing, for one.
    fn projection_for(&self, camera: &loom_render::Camera) -> gizmo::View {
        let extent = self.viewer.as_ref().map_or((1, 1), Viewer::extent);
        let placement = self.viewer.as_ref().and_then(Viewer::last_placement);
        #[allow(clippy::cast_precision_loss)]
        match placement {
            Some(p) => gizmo::View::at(
                camera,
                (p.x as f32, p.y as f32),
                p.width as f32,
                p.height as f32,
            ),
            None => gizmo::View::new(camera, extent.0 as f32, extent.1 as f32),
        }
    }

    fn pick_at_cursor(&mut self) {
        if self.viewer.is_none() {
            return;
        }
        let projection = self.projection();
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

    // **The one `unsafe` outside `loom_render*`, opened by name rather than by
    // opening the crate.** `loom_cli` denies `unsafe_code` at the workspace
    // level; this block is the documented exception, and it exists because
    // `ash_window::create_surface` needs the raw window handle and there is no
    // safe wrapper for it. Allowing it here rather than in `Cargo.toml` is the
    // difference between an exception and a hole.
    //
    // SAFETY: the window outlives the surface. `App`'s `Drop` destroys the
    // viewer — and with it this surface — before releasing the window. That is
    // written out explicitly there rather than left to field order, because
    // field order is what got this wrong the first time.
    #[allow(unsafe_code)]
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
    autoplay: bool,
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
    // A front end only makes sense in front of a game, so `--play` is what
    // arms it — and when it is armed it takes autoplay's job: the game starts
    // when the human asks for it, under the black.
    app.front = autoplay
        .then(|| app.view.world().camera_named(TITLE_CAM))
        .flatten()
        .map(|view| Front {
            shot: Camera {
                eye: Vec3::from_array(view.eye),
                target: Vec3::from_array(view.target),
                fov_y_degrees: view.fov_y_degrees,
            },
            // The top of the fade-up: the window opens black and lifts.
            clock: crate::hud::FADE_OUT + crate::hud::HOLD,
            leaving: false,
        });
    app.autoplay = autoplay && app.front.is_none();
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop failed: {e}"))
}

/// Load a scene and open it in a window.
///
/// # Errors
/// A message describing what stopped it.
pub fn open_scene(
    path: &str,
    editable: bool,
    frames: Option<u32>,
    autoplay: bool,
) -> Result<(), String> {
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

    run(path, view, session, disk_seen, frames, autoplay)
}

#[cfg(test)]
mod tests {
    use super::{
        CODE_ALPHABET, CODE_DENY, CODE_SYMBOLS, Escape, egui, encode, escape_means, format_code,
        generate_code, normalise, room_code_panel, wholesome,
    };

    /// A code is twelve base32 symbols in three groups of four, and every
    /// glyph on it is one a person can read down a voice channel.
    #[test]
    fn a_room_code_is_four_four_four_of_crockford_base32() {
        for _ in 0..64 {
            let code = generate_code().expect("the OS has entropy");
            assert_eq!(code.len(), CODE_SYMBOLS + 2, "{code} is not XXXX-XXXX-XXXX");
            assert_eq!(code.as_bytes()[4], b'-', "{code} has no group break at 4");
            assert_eq!(code.as_bytes()[9], b'-', "{code} has no group break at 9");
            for c in code.chars().filter(|c| *c != '-') {
                assert!(
                    CODE_ALPHABET.contains(&(c as u8)),
                    "{code} carries {c:?}, which is not in the alphabet"
                );
                assert!(
                    !"ILOU".contains(c),
                    "{code} carries {c:?}, which is exactly what Crockford excludes"
                );
            }
        }
    }

    /// **Twelve symbols of five bits each, and every one of them live.**
    ///
    /// The teeth are in the loop below: change the encoder's stride from 5 to
    /// 4 and it reddens with `symbol 10 is dead`. That is the bug class that
    /// matters, because a code with a stuck symbol still *looks* like a code.
    ///
    /// The `u64::MAX` line is weaker than it reads and is kept as a statement
    /// of intent rather than as a guard: with `& 31` inside the loop, `i` tops
    /// out at 11, so bits 60–63 are unreachable whatever any caller does, and
    /// the equality is a tautology for this shape. It says out loud that a
    /// code carries 60 bits and that the top nibble of a `u64` is discarded on
    /// purpose.
    #[test]
    fn the_encoder_reads_sixty_bits_and_not_one_more() {
        assert_eq!(format_code(u64::MAX), format_code((1 << 60) - 1));
        assert_eq!(format_code(0), "0000-0000-0000");
        assert_eq!(format_code(31), "0000-0000-000Z");
        // Every symbol position is live: 5 bits apart, 12 of them.
        for i in 0..CODE_SYMBOLS {
            assert_ne!(format_code(1 << (i * 5)), format_code(0), "symbol {i} is dead");
        }
    }

    /// **The property the whole thing rests on: a code parses back to itself.**
    /// Ten thousand of them, from a fixed seed so a failure is reproducible
    /// rather than a story about last Tuesday.
    #[test]
    fn every_code_round_trips_through_normalise() {
        let mut seed = 0x10AD_C0DE_u64;
        for _ in 0..10_000 {
            let code = format_code(splitmix(&mut seed));
            assert_eq!(normalise(&code).as_deref(), Some(code.as_str()));
        }
        // And the ones that came from the OS, not from the test's own PRNG.
        for _ in 0..16 {
            let code = generate_code().expect("the OS has entropy");
            assert_eq!(normalise(&code).as_deref(), Some(code.as_str()));
        }
    }

    /// **The alphabet can spell things, and the ones it can spell are the
    /// ones without an `I`, `L`, `O` or `U` in them.**
    ///
    /// Two halves, because a sampling test alone cannot see this: at 1 in
    /// 6,132 a hundred draws would pass on a denylist that had been emptied.
    /// So the predicate is checked against strings built to trip it, and the
    /// generator is checked to run through it.
    #[test]
    fn a_code_is_never_something_you_would_rather_not_read_out() {
        for bad in ["RAPE-0000-0000", "0000-0WAN-K000", "000K-KK00-0000"] {
            assert!(!wholesome(bad), "{bad} should have been rerolled");
        }
        for fine in ["0123-4567-89AB", "ZZZZ-ZZZZ-ZZZZ", "0000-0000-0000"] {
            assert!(wholesome(fine), "{fine} was rejected for nothing");
        }
        // Every entry is spellable — an entry carrying I, L, O or U can
        // never fire and is a comment pretending to be code.
        for bad in CODE_DENY {
            assert!(
                bad.bytes().all(|b| CODE_ALPHABET.contains(&b)),
                "{bad} cannot occur in a code at all"
            );
        }
        for _ in 0..64 {
            let code = generate_code().expect("the OS has entropy");
            assert!(wholesome(&code), "{code} came out of the generator");
        }
    }

    /// What a code survives between one mouth and another keyboard: any case,
    /// separators anywhere, and the four excluded letters heard for what the
    /// speaker meant. None of `I L O U` can be in a generated code, so mapping
    /// them can only ever rescue a typo.
    #[test]
    fn a_code_read_aloud_still_resolves() {
        // Chosen so the code carries a 1, a 0 and a V — the three symbols the
        // four excluded letters have to be heard as.
        let code = format_code(0x0007_6A5B_1AE7_C232);
        assert_eq!(code, "01VA-BCDE-FGHJ");

        let spoken = code
            .replace('1', "I")
            .replace('0', "O")
            .replace('V', "U")
            .to_lowercase();
        assert_eq!(normalise(&spoken).as_deref(), Some(code.as_str()));

        // Pasted out of a chat window one keypress at a time.
        let scattered: String = code.chars().flat_map(|c| [c, ' ']).collect();
        assert_eq!(normalise(&scattered).as_deref(), Some(code.as_str()));
        assert_eq!(normalise("  ").as_deref(), None);
    }

    /// Malformed input is refused rather than guessed at. A code that is one
    /// symbol short is not a code; padding it would join the wrong room.
    #[test]
    fn normalise_refuses_what_is_not_a_code() {
        for bad in [
            "",
            "ABCD-ABCD-ABC",   // eleven
            "ABCD-ABCD-ABCDE", // thirteen
            "ABCD-ABCD-ABC!",  // punctuation
            "ABCD-ABCD-ABC\u{e9}",
            "ABCD ABCD ABCD ABCD",
        ] {
            assert_eq!(normalise(bad), None, "{bad:?} was accepted as a code");
        }
    }

    /// **The collision claim, observed rather than only algebraed.**
    ///
    /// The design rests on `P ≈ N²/(2·A^L)`, and at `A = 32, L = 12` that
    /// probability is far too small to see in any test that finishes. So it is
    /// measured at a length where it bites — three symbols, 32,768 codes — and
    /// checked against the same closed form, then checked again one symbol
    /// longer to show the `A^L` in the denominator doing its work.
    #[test]
    fn the_birthday_bound_predicts_measured_collisions() {
        /// Fraction of `trials` draws of `n` codes of `symbols` length that
        /// contained a repeat.
        fn measured(symbols: usize, n: usize, trials: usize, seed: &mut u64) -> f64 {
            let mut hits = 0.0;
            for _ in 0..trials {
                let mut seen = std::collections::BTreeSet::new();
                if !(0..n).all(|_| seen.insert(encode(splitmix(seed), symbols))) {
                    hits += 1.0;
                }
            }
            hits / f64::from(u32::try_from(trials).unwrap())
        }
        /// `1 - exp(-n²/2M)`, the standard birthday approximation.
        fn predicted(symbols: i32, n: f64) -> f64 {
            1.0 - (-(n * n) / (2.0 * 32f64.powi(symbols))).exp()
        }

        let mut seed = 0xB16D_5EED_u64;
        for (symbols, n) in [(3_i32, 181_u32), (4, 724)] {
            let want = predicted(symbols, f64::from(n));
            let got = measured(
                usize::try_from(symbols).unwrap(),
                usize::try_from(n).unwrap(),
                400,
                &mut seed,
            );
            assert!(
                (got - want).abs() < 0.08,
                "{n} codes of {symbols} symbols collided {got:.3} of the time, \
                 against a predicted {want:.3}"
            );
        }

        // The scaling itself: one more symbol at the same population is 32x
        // rarer, which is the whole reason twelve is enough.
        let mut seed = 0x5EED_u64;
        let short = measured(3, 181, 400, &mut seed);
        let long = measured(4, 181, 400, &mut seed);
        assert!(
            long * 4.0 < short,
            "a longer code collided {long:.3} against {short:.3} — no scaling"
        );
    }

    /// SplitMix64. Test-only, so the collision measurement is reproducible;
    /// nothing in the engine draws from it.
    fn splitmix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// **The only proof available that the code is on screen**, since nothing
    /// in this workflow may open a window: lay the pause menu and the code
    /// block out through a real `egui::Context` and read the shapes back.
    ///
    /// Both are drawn together, because the claim is not just "the code is
    /// painted" but "it is painted top *right*, clear of the centred menu".
    #[test]
    fn the_pause_menu_shows_the_room_code_in_its_top_right() {
        const WIDTH: f32 = 1000.0;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(WIDTH, 600.0),
            )),
            ..egui::RawInput::default()
        };
        let code = format_code(0x0123_4567_89AB_CDEF);

        // Twice: the first pass of a fresh context is a layout pass and areas
        // have no size yet, exactly as the HUD's own tests found.
        let mut labels: Vec<(String, egui::Pos2, usize)> = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(input.clone(), |root| {
                let _ = crate::hud::pause_menu(root);
                room_code_panel(root, &code, &[]);
            });
            labels = out
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(t) => {
                        Some((t.galley.text().to_owned(), t.pos, t.galley.rows.len()))
                    }
                    _ => None,
                })
                .collect();
        }

        let found = |wanted: &str| {
            labels
                .iter()
                .find(|(text, ..)| text == wanted)
                .unwrap_or_else(|| panic!("the menu drew {labels:?}, with no {wanted:?} on it"))
        };
        let at = |wanted: &str| found(wanted).1;
        for pos in [at("ROOM CODE"), at(&code)] {
            assert!(
                pos.x > WIDTH * 0.5,
                "the room code was drawn at {pos:?}, in the left half of the view"
            );
            assert!(pos.y < 100.0, "the room code was drawn at {pos:?}, not near the top");
        }
        assert!(
            at("PAUSED").x < at("ROOM CODE").x,
            "the code should sit clear of the centred menu"
        );
        // **`Galley::text()` returns the source string, not what was laid
        // out**, so every assertion above passes just as happily on a code
        // wrapped across two ragged lines — which is what `CODE_WIDTH = 200`
        // actually drew. A code read aloud off two lines is precisely the
        // failure the Crockford alphabet exists to prevent, so the row count
        // is the assertion that matters most here.
        assert_eq!(
            found(&code).2,
            1,
            "{code} wrapped: CODE_WIDTH is too narrow for twelve glyphs and two dashes"
        );

        // **And it steps below whatever the HUD already put in that corner.**
        // `proving_ground.loom`'s kill counter is a `top_right` element that
        // survives the pause, and the code used to land straight across it.
        let clear = at(&code);
        let occupied = egui::Rect::from_min_max(
            egui::pos2(WIDTH - 125.0, 16.0),
            egui::pos2(WIDTH - 22.0, 41.0),
        );
        let mut moved = egui::Pos2::ZERO;
        for _ in 0..2 {
            let out = ctx.run_ui(input.clone(), |root| {
                room_code_panel(root, &code, &[occupied]);
            });
            moved = out
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(t) if t.galley.text() == code => Some(t.pos),
                    _ => None,
                })
                .expect("the code was not drawn at all");
        }
        assert!(
            moved.y > clear.y,
            "the code stayed at {moved:?} with the corner occupied — it was at {clear:?} empty"
        );
        assert!(
            moved.y > occupied.bottom(),
            "the code at {moved:?} is still inside the HUD's {occupied:?}"
        );
    }

    /// **The Escape state machine, which is the whole of the bug report.**
    /// "When I hit escape, it takes me back to the editor type view. And then
    /// if I hit it again, it just closes everything." Both halves of that are
    /// the `Close` row below reached twice over, and neither is reachable now
    /// while a character is being driven.
    #[test]
    fn escape_pauses_while_playing_and_closes_otherwise() {
        // (menu_open, playing, captured)
        for (state, want) in [
            ((false, true, true), Escape::Pause),   // driving: put the menu up
            ((true, true, true), Escape::Resume),   // menu up: take it down
            ((true, false, false), Escape::Resume), // Stop under an open menu
            ((false, false, false), Escape::Close), // just the viewer
            ((false, true, false), Escape::Close),  // playing, clicked out
        ] {
            let (menu_open, playing, captured) = state;
            assert_eq!(
                escape_means(menu_open, playing, captured),
                want,
                "escape_means{state:?}"
            );
        }
    }

    /// A second Escape must never close the window from inside the menu — that
    /// was the reported "it just closes everything", and it is the one
    /// transition worth spelling out as a sequence rather than as a table row.
    #[test]
    fn two_escapes_while_driving_pause_and_resume_rather_than_quitting() {
        let mut menu = false;
        let mut seen = Vec::new();
        for _ in 0..4 {
            let action = escape_means(menu, true, true);
            seen.push(action);
            match action {
                Escape::Pause => menu = true,
                Escape::Resume => menu = false,
                Escape::Close => break,
            }
        }
        assert_eq!(
            seen,
            [Escape::Pause, Escape::Resume, Escape::Pause, Escape::Resume],
            "Escape closed the window from inside a game"
        );
    }
    /// **The demo has a front end, and no other gate in this project can see
    /// that it does.** The title camera is found by name and is deliberately
    /// invisible to every other path — so it renders no pixel, moves no
    /// physics hash and fails no assertion if somebody renames it, reparents
    /// it or deletes it. The failure is the feature quietly not existing,
    /// which is the class of bug the golden gate has been fooled by twice.
    ///
    /// The second half is the load-bearing one: reading the spare must not
    /// have made it the scene's. If `active_camera` ever returns the title
    /// shot, `player_character` walks up from a node with no controller above
    /// it and the demo becomes unplayable.
    #[test]
    fn the_demo_opens_on_a_camera_the_game_never_uses() {
        let src = std::fs::read_to_string("../../assets/games/deeper_demo.loom")
            .expect("the demo scene");
        let scene = loom_scene::Scene::parse(&src).expect("it validates");
        let world = loom_ecs::World::from_scene(&scene);

        let shot = world
            .camera_named(super::TITLE_CAM)
            .unwrap_or_else(|| panic!("{} is gone, and with it the title screen", super::TITLE_CAM));
        assert!(
            (shot.fov_y_degrees - 42.0).abs() < f32::EPSILON,
            "the title shot lost its lens: {}",
            shot.fov_y_degrees
        );
        // Looking back along `Environment.sun_direction`, which is what puts
        // the glitter path under the word. A shot turned away from the sun is
        // duller than the frame it cuts to.
        let toward_sun = [0.42_f32, 0.46, -0.78];
        let forward = [
            shot.target[0] - shot.eye[0],
            shot.target[1] - shot.eye[1],
            shot.target[2] - shot.eye[2],
        ];
        let horizontal = |v: [f32; 3]| {
            let length = v[0].hypot(v[2]);
            [v[0] / length, v[2] / length]
        };
        let (a, b) = (horizontal(forward), horizontal(toward_sun));
        let cosine = b[1].mul_add(a[1], a[0] * b[0]);
        assert!(cosine > 0.95, "the title shot points away from the sun: {cosine}");

        let eye = world.active_camera().expect("the player still has one");
        assert!(
            (eye.fov_y_degrees - 75.0).abs() < f32::EPSILON,
            "the title camera became the scene's: {}",
            eye.fov_y_degrees
        );
    }
}
