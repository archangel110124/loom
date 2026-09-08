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

    /// Move along the view direction — ADR 0098.
    ///
    /// **Scaled by how far the pivot is**, so one notch crosses a sensible
    /// fraction of what you are looking at whether that is a doorknob or a
    /// harbour. A fixed step is unusable at both ends.
    fn dolly(&mut self, notches: f32, pivot_distance: f32) {
        let step = (pivot_distance * 0.12).clamp(0.05, 40.0);
        self.position += self.forward() * notches * step;
    }

    /// Slide the eye across the view plane, the way a middle-drag does
    /// everywhere else — ADR 0098.
    fn pan(&mut self, dx: f32, dy: f32, pivot_distance: f32) {
        // Pixels to metres at the pivot's depth, so the point under the cursor
        // keeps up with it rather than sliding away.
        let scale = (pivot_distance * 0.0022).clamp(0.002, 0.5);
        let up = self.right().cross(self.forward()).normalize_or_zero();
        self.position += self.right() * -dx * scale + up * dy * scale;
    }

    /// Turn around a point instead of on the spot — ADR 0098.
    ///
    /// **The one navigation verb a fly camera cannot fake.** Looking at a thing
    /// from another side means orbiting it; without this the only way round an
    /// object is to fly past and turn back, and you lose it off-screen doing so.
    fn orbit(&mut self, pivot: Vec3, dyaw: f32, dpitch: f32) {
        let offset = self.position - pivot;
        let radius = offset.length();
        if radius < 1e-4 {
            return;
        }
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-1.53, 1.53);
        // Back out along the new facing: the eye stays the same distance from
        // the pivot and keeps looking at it.
        self.position = pivot - self.forward() * radius;
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
/// — so where it sits in the file changes no pixel and no gameplay assertion,
/// both measured.
///
/// **It does move `World::state_hash`, and saying otherwise once was wrong.**
/// That hash eats every entity's name and `GlobalTransform` whether or not it
/// has a rigid body (see `a_plain_node_script_moves_its_own_transform`), so
/// adding two nodes to a scene changes it and reordering them changes it again.
/// Nothing pins this scene's hash and `xtask` compares a scene against itself,
/// so nothing here is broken — but "adding a camera cannot affect determinism"
/// is the kind of sentence someone builds on later, and it is false.
const TITLE_CAM: &str = "Rig/TitleCamera";

/// The front end: the shot it opens on, and the curtain over it.
///
/// **Rendering only**, and this struct genuinely is: three numbers read once at
/// load plus a wall clock, none of it in the world, none of it stepped by the
/// fixed step, and the curtain is one rectangle in an overlay no headless path
/// constructs. ADR 0045's line is nowhere near it. The *node* the numbers come
/// from is a different question — see [`TITLE_CAM`].
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
/// Menus, in a context of their own — ADR 0087.
///
/// **Not `fly` or `play`**: the same stick that walks a character must move a
/// highlight when a menu is up, and one action name cannot mean two things at
/// one moment. A context is exactly the tool this crate already had for that.
const MENU: &str = "menu";
/// What Play prints when it hands a character over.
///
/// One string rather than one per call site, because it is the only place
/// the key list is written down for a player at runtime and it went stale
/// once already: it still said "Esc frees the pointer" after Escape became
/// the pause menu.
///
/// **It is the engine's list, in the engine's words** — `fire` is a channel
/// name, and in the demo that channel casts a rod. A scene that wants to tell
/// a player what its verbs actually do writes a `Hud` line with
/// `only_on_title`, which is how `deeper_demo` puts its controls on the title
/// screen without `loom_cli` ever learning the word "cast".
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

/// What a script asked this window to do, when a script rather than a human is
/// driving it.
///
/// **This exists because nothing in the project could photograph a `Ui`.**
/// `loom render` is headless and never builds an egui context, so the HUD, the
/// pause menu, the title screen, the room code and the inventory grid were
/// visible to no gate: a HUD row shipped unreadable — pale text on pale deck —
/// and passed every check, and the inventory's hold-to-confirm bar drew for one
/// tick in sixty with nothing able to see it. The only check available was
/// counting the `Shape`s egui emitted, which proves a shape was produced and
/// says nothing about whether it was legible, on-screen or the right colour.
///
/// The three fields travel together and mean one thing between them, which is
/// why they are a struct rather than three more positional arguments on
/// [`open_scene`]: a run with any of them set is a run with no human at the
/// keyboard.
#[derive(Default)]
pub struct Script {
    /// Where to write a PNG of the last frame drawn — scene *and* overlay.
    pub shot: Option<std::path::PathBuf>,
    /// Scheduled player input: `loom sim`'s `--hold` tape, unchanged.
    pub hold: Vec<(u64, loom_script::Motion)>,
    /// Put the pause menu up as soon as play starts.
    pub menu: bool,
    /// Select this node on open — ADR 0099.
    ///
    /// **So a screenshot can show a selection.** Everything else about the
    /// editor is reachable headlessly; picking a node was mouse-only, which
    /// made the one thing a human does first the one thing no gate row and no
    /// agent could set up.
    pub select: Option<String>,
    /// Which gizmo tool to start in — ADR 0099. Move, Rotate or Scale.
    ///
    /// Same reason as `select`: the rotation rings only exist in Rotate mode,
    /// so without this no screenshot and no gate row could ever see them.
    pub mode: Option<String>,
    /// Which tab to bring to the front — ADR 0100. The last mouse-only thing.
    pub tab: Option<String>,
}

impl Script {
    /// Whether a script is driving the *game*, as opposed to merely taking a
    /// picture of whatever the window would have shown anyway.
    ///
    /// Two consequences, and both are deliberate:
    ///
    /// - **No title screen.** A scripted run cannot click Start — the title's
    ///   two controls are egui buttons and there is no key for them — so it
    ///   would sit on the front end until its frame budget ran out. Skipping it
    ///   drops the run into the existing autoplay path, which is the same door
    ///   Start goes through. A `--shot` with no `--hold` and no `--menu` leaves
    ///   the title up, which is how the title screen gets photographed.
    /// - **Hands off the pointer.** `start_play` grabs the cursor for
    ///   first-person look, and a background screenshot that steals the mouse
    ///   from whoever is at the machine is not acceptable. A script has no
    ///   mouse to look with, so it loses nothing.
    fn driving(&self) -> bool {
        !self.hold.is_empty() || self.menu
    }
}

/// Whether `run` should put a front end up at all.
///
/// **A free function for [`escape_means`]'s reason** — the arming lives inside
/// `run`, which needs an event loop — and because it was wrong: it read
/// `autoplay` alone, and `--edit --play` is a legal invocation, so a scene with
/// a title camera opened its title screen over the editor's docks. Nobody saw
/// it because no gate runs `--edit --play` on a scene that has one; `xtask
/// validate`'s CPU-budget block runs exactly that shape, and would have
/// measured a menu.
const fn front_end_wanted(autoplay: bool, editing: bool) -> bool {
    // `--play` and nothing else. In the editor the human already has Play, and
    // a curtain over a dock hides the panels rather than the game.
    autoplay && !editing
}

/// Whether the title screen's buttons should be listened to yet, `clock`
/// seconds into [`Front`]'s timeline.
///
/// **The curtain hides the menu and does not disarm it.** [`crate::hud::fade`]
/// paints, and a painter takes no input, so Start and Quit are live from the
/// first frame — under an opening curtain that begins at 99% black. Answering a
/// click there took the screen from black to the full shot in one frame and
/// then faded it out again, and answering Quit closed the window from behind
/// black with nothing to show for it.
///
/// The buttons are still *drawn* the whole time, so nothing pops in when this
/// turns true; only the answer waits.
fn title_answers(clock: f32) -> bool {
    clock >= crate::hud::TRANSITION
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
    /// Every node this drag moves, with the transform each had when it began —
    /// ADR 0098.
    ///
    /// **A multi-selection used to move one node.** The gizmo captured only the
    /// focused path, so selecting six crates and dragging moved the one you
    /// happened to click last. Each node carries its own start for the same
    /// reason `start` exists: absolute from the beginning, so a dropped frame
    /// costs nothing.
    group: Vec<(String, [[f32; 3]; 3])>,
    /// The gizmo's centre in window pixels, and the angle the press was at —
    /// ADR 0099. Only meaningful for a ring drag.
    centre: (f32, f32),
    from_angle: f32,
    /// True when the press landed on a rotation ring rather than an axis line.
    ring: bool,
    /// Which way that ring turns from where the camera was — ADR 0099.
    sign: f32,
    /// The second axis, when the press landed on a plane quad — ADR 0099.
    also: Option<gizmo::Handle>,
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
    /// Increment snapping for the gizmo — ADR 0093. Off by default; Ctrl
    /// inverts whatever it is set to.
    snap: gizmo::Snap,
    /// What the viewport draws — ADR 0097.
    view_mode: loom_render::ablate::ViewMode,
    /// What the hierarchy is filtered to — ADR 0093. UI state, so it lives
    /// here rather than in the scene.
    hierarchy_filter: String,
    /// Nodes whose children are folded away — ADR 0098.
    collapsed: std::collections::BTreeSet<String>,
    /// Where the last pick landed, so clicking the same spot cycles through
    /// what is stacked there — ADR 0099.
    last_pick: Option<(f32, f32)>,
    /// The node being renamed in the hierarchy, if any — ADR 0100.
    renaming: Option<String>,
    /// The conversation with the agent, re-read whenever the scene is polled —
    /// ADR 0100.
    agent_chat: Vec<loom_editor::AgentTurn>,
    /// True while something we asked has no answer yet.
    agent_busy: bool,
    /// Rotation rings in window pixels, recomputed with the handles so a press
    /// hit-tests exactly what was drawn — ADR 0099.
    rings: Vec<(usize, Vec<(f32, f32)>)>,
    /// Plane quads, for moving in two axes at once — ADR 0099.
    planes: Vec<gizmo::Plane>,
    /// What is wrong with the scene, recomputed on change — ADR 0093.
    problems: Vec<loom_editor::Problem>,
    /// The prefabs the *unresolved* scene declares — ADR 0101.
    prefabs: Vec<loom_editor::PrefabRow>,
    /// Every edit from outside this window, newest last — ADR 0093.
    ///
    /// **Separate from `agent_changes`, which fades in six seconds.** That is
    /// right for a viewport overlay and useless as a record: an agent that
    /// edited eleven nodes while the human read the inspector used to leave
    /// nothing behind to review.
    agent_log: Vec<(crate::scene_view::Change, std::time::Instant)>,
    /// Scene files beside the open one — ADR 0093.
    ///
    /// **Cached, not listed per frame.** A `read_dir` at 144 Hz to draw a row
    /// of buttons is a filesystem call per frame for a list that changes when
    /// somebody adds a file.
    scenes: Vec<String>,
    /// Copied subtrees, one per selected node — ADR 0093.
    ///
    /// **The nodes, not their paths.** A clipboard holding paths would paste
    /// nothing after the original was deleted, which is exactly when somebody
    /// reaches for cut-and-paste.
    clipboard: Vec<Vec<loom_scene::Node>>,
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
    /// Gamepads, when the platform has any — ADR 0086. `None` on a headless
    /// box, in a container with no `/dev/input`, or on CI, none of which should
    /// stop a scene from opening.
    gamepads: Option<loom_input::Gamepads>,
    /// Which menu item is selected, and whether anything is — ADR 0087.
    menu_focus: crate::hud::MenuFocus,
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
    /// Smoothed per-frame cost, milliseconds — ADR 0093.
    ///
    /// **The same two numbers `--frames` prints on the way out**, live in the
    /// status bar. Until now the only way to see which half of a frame to fix
    /// was to close the editor and read the terminal.
    cpu_ms: f32,
    draw_ms: f32,
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
    /// What a script asked for, or all defaults when a human is driving.
    script: Script,
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
        // Before the struct takes `view`: the scene names its own scheme.
        let bindings = load_bindings(&view);
        let gamepads = match loom_input::Gamepads::open() {
            Ok(pads) => Some(pads),
            Err(e) => {
                // Reported once, at startup, rather than per frame: a machine
                // with no pads is the normal case and must not be noisy.
                eprintln!("loom: no gamepad support ({e}); keyboard and mouse only");
                None
            }
        };
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
            snap: gizmo::Snap::default(),
            view_mode: loom_render::ablate::ViewMode::default(),
            hierarchy_filter: String::new(),
            collapsed: std::collections::BTreeSet::new(),
            last_pick: None,
            renaming: None,
            agent_chat: Vec::new(),
            agent_busy: false,
            rings: Vec::new(),
            planes: Vec::new(),
            clipboard: Vec::new(),
            scenes: Vec::new(),
            problems: Vec::new(),
            prefabs: Vec::new(),
            agent_log: Vec::new(),
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
            bindings,
            input: InputState::new(),
            gamepads,
            menu_focus: crate::hud::MenuFocus::default(),
            window: None,
            viewer: None,
            dock: None,
            gpu: None,
            #[allow(clippy::disallowed_methods)]
            last_frame: std::time::Instant::now(),
            wind_seconds: 0.0,
            fps: 0.0,
            cpu_ms: 0.0,
            draw_ms: 0.0,
            frames_left: None,
            script: Script::default(),
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
            //
            // **Subtree bounds, so a rig node is markable.** An agent editing
            // `Rig/Boat` drew no mark at all, because the node it named has no
            // mesh of its own — the change happened and nothing pointed at it.
            let Some((lo, hi)) = self.subtree_bounds(&change.path) else {
                continue;
            };
            let bounds = loom_scene::place::Bounds { min: lo.to_array(), max: hi.to_array() };
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
            self.agent_log
                .extend(self.agent_changes.iter().map(|(c, at)| (c.clone(), *at)));
            // A session that ran all evening should not carry every edit it
            // ever saw; the panel shows the recent ones and that is what it is
            // for.
            const KEEP: usize = 200;
            if self.agent_log.len() > KEEP {
                let excess = self.agent_log.len() - KEEP;
                self.agent_log.drain(0..excess);
            }
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
        self.recompute_problems();
        self.recompute_prefabs();
        self.scenes = self.sibling_scenes();
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
        // The conversation, on the same beat as the file — ADR 0100.
        self.refresh_agent_chat();

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
            // **The subtree, not the node** — the same fix the gizmo needed.
            // A rig node carries a transform and no mesh, so framing its own
            // bounds found nothing and fell through to the whole scene: F on
            // `Rig/Boat` framed the harbour instead of the boat.
            if let Some((lo, hi)) = self.subtree_bounds(path) {
                min = min.min(lo);
                max = max.max(hi);
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
                            // An asked-for tab, once the dock exists.
                            if let (Some(dock), Some(wanted)) =
                                (self.dock.as_mut(), self.script.tab.as_deref())
                                && let Some(tab) = loom_editor::Tab::from_title(wanted)
                            {
                                dock.focus(tab);
                            }
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

            // **Scroll dollies** — ADR 0098. The single most-used input in any
            // 3D tool, and there was no handler for it at all.
            WindowEvent::MouseWheel { delta, .. } => {
                if self.ui.as_ref().is_some_and(Ui::wants_pointer) {
                    return;
                }
                let notches = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    // A touchpad reports pixels; 40 of them is about a notch.
                    #[allow(clippy::cast_possible_truncation)]
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                let pivot = self.orbit_pivot();
                let distance = (self.camera.position - pivot).length();
                self.camera.dolly(notches, distance);
            }

            WindowEvent::CursorMoved { position, .. } => {
                let was = self.cursor;
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

                // **Middle-drag pans, Alt+left-drag orbits** — ADR 0098. Both
                // read the delta here rather than per frame, because a camera
                // move costs nothing to apply and should feel immediate.
                let delta = (self.cursor.0 - was.0, self.cursor.1 - was.1);
                let over_panel = self.ui.as_ref().is_some_and(Ui::wants_pointer);
                if !over_panel {
                    let pivot = self.orbit_pivot();
                    let distance = (self.camera.position - pivot).length();
                    if self.input.held("MouseMiddle") {
                        self.camera.pan(delta.0, delta.1, distance);
                    } else if self.input.held("MouseLeft")
                        && (self.input.held("AltLeft") || self.input.held("AltRight"))
                    {
                        self.camera.orbit(pivot, delta.0 * -0.006, delta.1 * -0.006);
                    }
                }
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
                // **Drained here, before anything reads a binding.** Both
                // `step_camera` and the play input path consult the action map
                // further down this frame; an event pumped after either of them
                // waits a whole frame, which on a stick is stale deflection and
                // on a button is a press the player made and did not get.
                if let Some(pads) = self.gamepads.as_mut() {
                    pads.pump(&mut self.input);
                }
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
                    // `--menu`: the pause menu has no key a script can press —
                    // Escape means *close* to a run with no captured pointer
                    // (`escape_means(false, true, false)`), so scripting the
                    // key would shut the window rather than photograph the
                    // menu. Asking for the state directly is both smaller and
                    // the only thing that works.
                    if self.script.menu {
                        self.set_pause_menu(true);
                    }
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
                let editing_now = self.session.is_some() && self.play.is_none();
                let focused_bounds = editing_now
                    .then(|| self.focused())
                    .flatten()
                    .and_then(|path| self.subtree_bounds(&path));
                let centre = focused_bounds.map(|(min, max)| (min + max) * 0.5);
                self.handles = centre.map_or_else(Vec::new, |c| gizmo::handles(&projection, c));
                // A ring at a constant size on screen — see `gizmo::rings`.
                // Sizing it to the object put a circle bigger than the window
                // across the whole viewport.
                self.planes = if self.mode == Mode::Move {
                    gizmo::planes(&self.handles)
                } else {
                    Vec::new()
                };
                self.rings = match (centre, self.mode) {
                    (Some(c), Mode::Rotate) => gizmo::rings(&projection, c),
                    _ => Vec::new(),
                };

                // Before `drawn` borrows the object list: this prunes the
                // faded entries, so it needs `&mut self`.
                let marks = self.agent_marks(&projection, now);
                let selection_edges = self.selection_edges(&projection);


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
                // Newest first, which is the order somebody reviewing asks for.
                let agent_log: Vec<loom_editor::AgentEdit> = self
                    .agent_log
                    .iter()
                    .rev()
                    .map(|(change, at)| loom_editor::AgentEdit {
                        node: change.path.clone(),
                        kind: change.kind.label().to_owned(),
                        seconds_ago: now.duration_since(*at).as_secs_f32(),
                    })
                    .collect();
                let state = PanelState {
                    rings: &self.rings,
                    planes: &self.planes,
                    selection_edges: &selection_edges,
                    snap: self.snap,
                    view_mode: self.view_mode,
                    problems: &self.problems,
                    prefabs: &self.prefabs,
                    scenes: &self.scenes,
                    open_scene: self.scene_path.to_str().unwrap_or_default(),
                    cpu_ms: self.cpu_ms,
                    draw_ms: self.draw_ms,
                    agent_log: &agent_log,
                    agent_chat: &self.agent_chat,
                    agent_busy: self.agent_busy,
                    redo_history: self
                        .session
                        .as_ref()
                        .map_or(&[][..], loom_scene::edit::Session::redo_labels),
                    filter: &self.hierarchy_filter,
                    collapsed: &self.collapsed,
                    renaming: self.renaming.as_deref(),
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
                // **`dread` comes from the running script, or from the
                // scene** — ADR 0069. Read here and never eased here:
                // `self.wind_seconds` advances by a frame delta, and a
                // mood riding that would pass the image gate, pass
                // `cargo xtask repeat`, and be wrong only in this window.
                #[allow(clippy::cast_possible_truncation)]
                let dread = self
                    .play
                    .as_ref()
                    .and_then(|p| p.state().number("dread"))
                    .map(|v| v as f32);
                // **The whole weather at that rung, once.** This window used
                // to take the ramped wind for the sea (inside
                // `environment_with_mood`) and the file's wind for the rain
                // and the submersion test beside it — streaks slanting at the
                // berth's angle over a gale sea, in the one place a human
                // judges weather. See `weather_at`.
                let (wind, rain) = crate::weather_at(
                    world,
                    crate::weather::rain_of(&self.view.scene),
                    dread,
                );
                let (mut environment, grade) =
                    crate::environment_with_mood(world, &wind, self.wind_seconds, dread);
                // **The window stamped neither engine-owned texture, and that
                // was a real hole rather than tidiness.** `loom render` has
                // called this since the fire flipbook landed; this path builds
                // its environment from scratch every frame and never did, so a
                // scene's flipbook — and now the sea's foam detail — reached
                // the offscreen PNG and not the window the human judges in.
                // The same defect class as the viewer drawing at one MSAA
                // sample: measuring the effect somewhere the effect is not.
                crate::stamp_engine_textures(&mut environment, self.view.materials());
                // Whether the eye is under the water, from the same query that
                // muffles the sound (W7). The fly camera and a swimming
                // character both go through here, so the window's view and the
                // scripts' `is_submerged` cannot disagree.
                crate::submerge_eye(
                    &mut environment,
                    world,
                    &wind,
                    self.play.as_ref().and_then(crate::play::Play::sea),
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
                    rain.as_ref(),
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
                let mut crowns: Vec<loom_render::ParticleInstance> = fluid_spray;
                // **And the crest spray, which this window has never drawn.**
                // `fluid_spray` above is the *cinematic* tier's readback alone
                // — ADR 0053's GPU-stateful path — so a `WaterBody` authoring
                // `spray = N` on the ordinary tiers threw nothing here, while
                // the headless still (`main.rs`, beside `water_of`) and
                // `--frames` (fixed in 592f605) both appended it. `loom run`
                // is what a human opens to judge the sea, so every judgement
                // ever made about spray was made on a view that was not
                // drawing it — the third path of the same defect, and the
                // reason it survived all four gates is that not one of them
                // photographs a window.
                //
                // After the camera and on `wind_seconds`, for the two reasons
                // the headless paths give: the population is bounded by
                // `SPRAY_RANGE` around the eye, and a crown frozen over a
                // moving sea is the artifact. `Play::sea` is the same accessor
                // the ocean upload below uses, and is `None` in edit mode —
                // where a spectrum body throws nothing, which is the honest
                // answer for a sea nobody has evolved.
                if let Some(body) = crate::weather::water_of(world, &wind) {
                    let ground = |x: f32, z: f32| {
                        self.terrain
                            .as_ref()
                            .map_or(loom_voxel::heightfield::NO_GROUND, |g| g.at(x, z))
                    };
                    crowns.extend(crate::particles::spray(
                        world,
                        &body,
                        self.play.as_ref().and_then(crate::play::Play::sea),
                        &wind,
                        &ground,
                        camera.eye.to_array(),
                        self.wind_seconds,
                    ));
                }
                let combined;
                let particles: &[loom_render::ParticleInstance] = if crowns.is_empty() {
                    particles
                } else {
                    combined = [particles, &crowns].concat();
                    &combined
                };
                if let Some(viewer) = self.viewer.as_mut() {
                    viewer.environment = environment;
                    viewer.grade = grade;
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
                    // **And the FFT cascade** — ADR 0076 — on exactly the rule
                    // the two fields above it follow. Without this the window
                    // draws a FLAT sea for a `spectrum` body while the physics
                    // runs the real one: such a body authors no waves, so the
                    // Gerstner sum the shader falls back to sums nothing, and
                    // a buoy heaves on a plane. Observed, not theorised.
                    //
                    // **No gate can catch that.** `validate`, `image`,
                    // `repeat`, `ablate` and `test` all go through the
                    // headless path, which was wired first; nothing in this
                    // repository photographs a window.
                    //
                    // Last tick's tile, not a re-evaluation at frame time. The
                    // cascade is stateless so re-evaluating would be legal —
                    // that is a real dividend of ADR 0076 — but it costs a
                    // full `Ocean::evolve` at 3.710 ms to move the surface
                    // 8.5 mm, and measured, the tick boundary does not judder.
                    if let Some(sea) = self.play.as_ref().and_then(crate::play::Play::sea) {
                        let tiles = sea.render_tiles();
                        if let Err(e) =
                            viewer.set_ocean(&tiles.tiles, &tiles.patch, &tiles.longest, tiles.n)
                        {
                            crate::log::warn(format!("ocean: {e}"));
                        }
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
                    // Smoothed the same way `fps` is, so the three numbers
                    // beside each other settle at the same rate.
                    self.cpu_ms = self.cpu_ms.mul_add(0.9, cpu_ms as f32 * 0.1);
                }
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
                // Read before the draw closure borrows `self.viewer`
                // mutably, the same way `menu_open` is — the closure
                // is `FnMut` and cannot hold a second borrow of self.
                let creel = self
                    .play
                    .as_ref()
                    .and_then(|play| crate::hud::Creel::read(play.state()));
                let menu_open = self.pause_menu;
                // The front end's three halves, read out like `menu_open` is:
                // whether the title's *text* is up, whether its *menu* is, and
                // how black the screen is.
                //
                // **They are not the same lifetime, and treating them as one
                // was visible.** The menu goes on the click, which is the right
                // feedback. The word does not: it is drawn for exactly as long
                // as the overlay is resolved against the un-played world, which
                // carries it through the half-second fade-out and swaps it for
                // the game's own rows under the black. Lifting the scrim with
                // the *menu* therefore un-dimmed a picture the word was still
                // written across — an 18% brightness step on the same frame as
                // the click, which is a flash where a fade was asked for. It
                // now goes with the word.
                let title_lines = self.play.is_none() && self.front.is_some();
                let title_up = self.front.as_ref().is_some_and(|f| !f.leaving);
                // **Resolved out here, because `self` is borrowed inside the
                // closure below.** The menus take a plain struct of booleans so
                // `hud` never learns what a gamepad is — the binding layer has
                // already turned a D-pad, a stick past its dead zone and the
                // arrow keys into the same three facts.
                let nav = crate::hud::MenuNav {
                    up: self.input.is_active(&self.bindings, MENU, "menu_up"),
                    down: self.input.is_active(&self.bindings, MENU, "menu_down"),
                    confirm: self.input.is_active(&self.bindings, MENU, "menu_confirm"),
                };
                let mut menu_focus = self.menu_focus;
                let curtain = self.front.as_ref().map(|f| crate::hud::curtain(f.clock));
                let room_code = self.room_code.as_deref();
                let mut pause_choice = None;
                let mut title_choice = None;
                // **`--shot` photographs the last frame of the budget, not the
                // first.** The last frame is the one the run has *arranged*:
                // the tape has played out, the menu is up, the fade is over.
                // Armed here, before the draw, because the capture is a pass
                // inside that draw — it copies the swapchain image after the
                // overlay has been composited into it, which is the only place
                // a picture of the HUD exists.
                //
                // A sequence — one PNG per frame, the way `loom render
                // --frames` writes them — is what an *animated* overlay needs,
                // and is deliberately not built: see the note on `--shot` in
                // `main.rs`.
                if self.frames_left == Some(1) {
                    let path = self.script.shot.take();
                    // **The geometry the shot actually came out at**, because a
                    // gate that measures a rectangle inside it cannot assume
                    // one. `with_inner_size` above is a *request*: a tiling
                    // compositor ignores it outright, and a fractional scale
                    // makes the swapchain bigger than the points egui laid the
                    // overlay out in — so neither the window's size nor the
                    // HUD's size in the PNG is knowable from the file alone.
                    // Both numbers, together, are. `scripts/green.sh` reads
                    // this line to place its band; see the overlay row there.
                    if path.is_some()
                        && let Some(window) = self.window.as_ref()
                    {
                        let px = window.inner_size();
                        let ppp = window.scale_factor();
                        crate::log::info(format!(
                            "shot {}x{} px at {ppp:.4} ppp",
                            px.width, px.height
                        ));
                    }
                    if let (Some(path), Some(viewer)) = (path, self.viewer.as_mut()) {
                        viewer.capture(path);
                    }
                }
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
                            if title_lines {
                                crate::hud::title_scrim(root);
                            }
                            // The rects are the second half of this return
                            // value and were thrown away until the room code
                            // needed somewhere to stand — see
                            // [`room_code_panel`].
                            let (_, painted) = crate::hud::draw(root, &overlay);
                            // **The creel, over the HUD and under the
                            // pause menu**, painted into the root `Ui`
                            // so it claims neither the pointer nor the
                            // keyboard. Draws nothing at all unless the
                            // running game exports a grid, which is
                            // every scene in this project but one.
                            if let Some(view) = creel.as_ref() {
                                let _ = crate::hud::creel(root, view);
                            }
                            if title_up {
                                title_choice =
                                    crate::hud::title_menu(root, nav, &mut menu_focus);
                            } else if menu_open {
                                pause_choice =
                                    crate::hud::pause_menu(root, nav, &mut menu_focus);
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
                let this_draw_ms = draw_started.elapsed().as_secs_f64() * 1000.0;
                self.draw_total_ms += this_draw_ms;
                #[allow(clippy::cast_possible_truncation)]
                {
                    self.draw_ms = self.draw_ms.mul_add(0.9, this_draw_ms as f32 * 0.1);
                }
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
                // Not while the opening is still lifting — see
                // [`title_answers`], which owns that reasoning and the test.
                let answerable = self
                    .front
                    .as_ref()
                    .is_some_and(|f| title_answers(f.clock));
                // **Written back, or the selection resets every frame** and the
                // highlight can never move: `step` would run on a fresh
                // `MenuFocus` each time and the menu would look inert while
                // reporting that it engaged.
                self.menu_focus = menu_focus;
                match title_choice.filter(|_| answerable) {
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
            UiAction::SetFilter(text) => self.hierarchy_filter = text,
            UiAction::SaveGame => self.save_game(),
            UiAction::LoadGame => self.load_game(),
            // Empty clears it — the panel says "done" that way rather than
            // needing a second verb.
            UiAction::BeginRename(path) => {
                self.renaming = (!path.is_empty()).then_some(path);
            }
            UiAction::ToggleCollapsed(path) => {
                if !self.collapsed.remove(&path) {
                    self.collapsed.insert(path);
                }
            }
            UiAction::SetViewMode(mode) => {
                self.view_mode = mode;
                if let Some(viewer) = self.viewer.as_mut() {
                    viewer.set_view_mode(mode);
                }
            }
            UiAction::SetSnap(on) => self.snap.enabled = on,
            UiAction::SetSnapStep(mode, step) => match mode {
                Mode::Move => self.snap.translate = step,
                Mode::Rotate => self.snap.rotate = step,
                Mode::Scale => self.snap.scale = step,
            },
            UiAction::Focus => {
                self.camera = FlyCamera::framing_at(self.focus_bounds(), self.camera.fov_y_degrees);
            }
            UiAction::AddChild(parent) => self.add_child(&parent),
            UiAction::AddPrefabInstance(key) => self.add_prefab_instance(&key),
            UiAction::CreatePrimitive(shape) => self.create_primitive(&shape),
            UiAction::OpenScene(path) => self.open_scene(&path),
            UiAction::SendToAgent(text) => self.ask_agent(&text),
            UiAction::DropAsset { alias, at } => self.drop_asset(&alias, at),
            UiAction::NewScene => self.new_scene(),
            UiAction::SaveAs(name) => self.save_as(&name),
            UiAction::Copy => self.copy_selection(),
            UiAction::Paste => self.paste_clipboard(),
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
        if drivable && !self.script.driving() {
            self.capture_pointer(true);
            crate::log::info(PLAY_KEYS);
        } else if drivable {
            // Scripted: the tape drives the character and the human keeps
            // their mouse. `feed_play_input` knows not to consult `captured`.
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
        // **A tape overrides the keyboard, and overrides the capture gate with
        // it.** A scripted run never grabs the pointer (see [`Script::driving`]),
        // so the `captured` test below would read every scripted run as hands
        // off the keys and nothing would ever move. Same schedule and the same
        // parser `loom sim --hold` uses — the point of reaching `loom run` with
        // it is that the inventory, the HUD and the rest of the overlay only
        // exist in a window.
        if !self.script.hold.is_empty() {
            let held = crate::input_at(&self.script.hold, u64::from(play.ticks));
            play.set_input(crate::play::PlayerInput {
                move_axis: held.move_axis,
                jump: held.jump,
                sprint: held.sprint,
                fire: held.fire,
                interact: held.interact,
                bag: held.bag,
            });
            return;
        }
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
        // **The ladder's rain, at this tick's rung.** Same `weather_at` the
        // picture goes through, so the bed cannot be raining harder or softer
        // than the streaks in front of it.
        #[allow(clippy::cast_possible_truncation)]
        let dread = play.state().number("dread").map(|v| v as f32);
        let rain = crate::weather_at(
            &play.world,
            crate::weather::rain_of(&self.view.scene),
            dread,
        )
        .1
        .map_or(0.0, |r| r.intensity);
        sound.set_rain(rain);
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
        // Guarded here as well as in the panel: every path into reparenting
        // goes through this one function, so this is where the rule cannot be
        // missed by a new call site.
        if self.play.is_some() {
            crate::log::warn("stop playing before moving a node in the hierarchy".to_owned());
            return;
        }
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
                self.reapply_to_play();
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

    /// Every node at or under `root`, shallowest first.
    ///
    /// **Shallowest first matters**: a child cannot be spawned before its
    /// parent exists, and `SpawnNode` is refused if the parent is missing.
    fn subtree(&self, root: &str) -> Vec<loom_scene::Node> {
        let prefix = format!("{root}/");
        let mut nodes: Vec<loom_scene::Node> = self
            .view
            .scene
            .nodes()
            .iter()
            .filter(|n| n.path == root || n.path.starts_with(&prefix))
            .cloned()
            .collect();
        nodes.sort_by_key(|n| n.path.matches('/').count());
        nodes
    }

    /// Ops that recreate `nodes` — a subtree captured by [`Self::subtree`] —
    /// underneath `parent`, with the root renamed to `name`.
    ///
    /// **One function for duplicate and for paste**, because they are the same
    /// operation with a different source. Duplicate used to spawn the node and
    /// none of its children, so duplicating a rig produced an empty rig; this is
    /// where that is fixed, once, for both.
    fn respawn_ops(
        nodes: &[loom_scene::Node],
        parent: &str,
        name: &str,
    ) -> (Vec<loom_scene::SceneOp>, String) {
        let mut ops = Vec::new();
        let Some(root) = nodes.first() else {
            return (ops, String::new());
        };
        let new_root = if parent.is_empty() {
            name.to_owned()
        } else {
            format!("{parent}/{name}")
        };

        for node in nodes {
            // Where this node lands: the root becomes `new_root`, and everything
            // under it keeps its shape.
            let new_path = if node.path == root.path {
                new_root.clone()
            } else {
                let tail = node.path.strip_prefix(&format!("{}/", root.path)).unwrap_or(&node.path);
                format!("{new_root}/{tail}")
            };
            let (into, leaf) = match new_path.rsplit_once('/') {
                Some((into, leaf)) => (into.to_owned(), leaf.to_owned()),
                None => (String::new(), new_path.clone()),
            };
            ops.push(loom_scene::SceneOp::SpawnNode {
                parent: into,
                name: leaf,
                mesh: None,
                // A prefab instance pastes as an instance, not as a flattened
                // copy of whatever it happened to expand to.
                prefab: node.prefab.clone(),
            });
            let t = &node.transform;
            ops.push(transform_op(
                &new_path,
                Some(t.pos),
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
            // Deviations from the prefab travel with the instance; a copy that
            // dropped them would silently revert to the prefab's values.
            for (field, value) in &node.overrides {
                ops.push(loom_scene::SceneOp::SetField {
                    node: new_path.clone(),
                    field: field.clone(),
                    value: value.clone(),
                });
            }
        }
        (ops, new_root)
    }

    /// A name like `name`, `nameCopy`, `nameCopy2` — whichever is free.
    fn free_name(&self, parent: &str, name: &str) -> String {
        let taken = |candidate: &str| {
            let path = if parent.is_empty() {
                candidate.to_owned()
            } else {
                format!("{parent}/{candidate}")
            };
            self.view.paths.contains(&path)
        };
        if !taken(name) {
            return name.to_owned();
        }
        let mut n = 1;
        loop {
            let candidate = if n == 1 {
                format!("{name}Copy")
            } else {
                format!("{name}Copy{n}")
            };
            if !taken(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }




    /// Scene files beside the open one, for the Project panel — ADR 0093.
    ///
    /// One directory, not a recursive walk: a project's scenes live together,
    /// and a walk that found every `.loom` under `assets/` would list 137 files
    /// of which 130 are test fixtures.
    fn sibling_scenes(&self) -> Vec<String> {
        let mut found = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.base) else {
            return found;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "loom")
                && let Some(text) = path.to_str()
            {
                found.push(text.to_owned());
            }
        }
        found.sort();
        found
    }

    /// Open a different scene — ADR 0093.
    ///
    /// **Refused while there are unsaved edits.** The alternative is a
    /// confirmation dialog, and the alternative to that is losing somebody's
    /// work to a misclick in a file list. Save, then switch.
    fn open_scene(&mut self, path: &str) {
        if self.dirty {
            crate::log::warn("save first — opening another scene would lose unsaved edits".to_owned());
            return;
        }
        // A prefab declares its path relative to the scene that names it, so
        // "open the prefab" resolves against the current base rather than the
        // working directory — which is what makes the Prefabs panel's Open
        // button work from anywhere.
        let candidate = std::path::Path::new(path);
        let path = if candidate.is_absolute() || candidate.exists() {
            candidate.to_path_buf()
        } else {
            self.base.join(candidate)
        };
        let text = match loom_asset::pack::read_text(&path) {
            Ok(text) => text,
            Err(e) => {
                crate::log::error(format!("{}: {e}", path.display()));
                return;
            }
        };

        // Everything keyed to the old file goes at once. A base left pointing
        // at the previous directory would resolve the new scene's meshes
        // against the wrong folder and silently substitute boxes.
        self.base = path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        self.scene_path = path.clone();
        self.session = Some(loom_scene::edit::Session::from_text(&path, text.clone()));
        // Play holds a world built from the scene that is going away.
        self.play = None;
        self.selected.clear();
        self.agent_log.clear();
        self.agent_changes.clear();
        self.show(&text);
        crate::log::info(format!("opened {}", path.display()));
    }

    /// Create a node that draws `shape`, under the selection — ADR 0093.
    ///
    /// One transaction, so it is one Ctrl+Z rather than three.
    fn create_primitive(&mut self, shape: &str) {
        let parent = self.selected.first().cloned().unwrap_or_default();
        let name = self.free_name(&parent, shape);
        let path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        self.transact(
            format!("Create {shape}"),
            vec![loom_scene::SceneOp::SpawnNode {
                parent,
                name,
                mesh: Some(shape.to_owned()),
                prefab: None,
            }],
        );
        self.selected = vec![path];
    }

    /// Spawn a prefab instance under the selection — ADR 0093.
    fn add_prefab_instance(&mut self, key: &str) {
        let parent = self.selected.first().cloned().unwrap_or_default();
        let name = self.free_name(&parent, key);
        let path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        self.transact(
            format!("Add {key} instance"),
            vec![loom_scene::SceneOp::SpawnNode {
                parent,
                name,
                mesh: None,
                prefab: Some(key.to_owned()),
            }],
        );
        self.selected = vec![path];
    }

    /// Put the selection on the clipboard, subtrees and all — ADR 0093.
    fn copy_selection(&mut self) {
        self.clipboard = self
            .selected
            .iter()
            .map(|path| self.subtree(path))
            .filter(|nodes| !nodes.is_empty())
            .collect();
        let count = self.clipboard.len();
        let nodes: usize = self.clipboard.iter().map(Vec::len).sum();
        crate::log::info(format!("copied {count} selection(s), {nodes} node(s)"));
    }

    /// Paste under the selection, or beside the original when nothing is
    /// selected.
    fn paste_clipboard(&mut self) {
        if self.clipboard.is_empty() {
            crate::log::warn("nothing on the clipboard".to_owned());
            return;
        }
        let clipboard = self.clipboard.clone();
        let mut ops = Vec::new();
        let mut created = Vec::new();
        for nodes in &clipboard {
            let Some(root) = nodes.first() else { continue };
            // Into the selection when there is one, so paste is "put it in
            // here"; otherwise beside where it came from.
            let parent = self.selected.first().cloned().unwrap_or_else(|| {
                root.path.rsplit_once('/').map_or(String::new(), |(p, _)| p.to_owned())
            });
            let name = self.free_name(&parent, &root.name);
            let (mut node_ops, new_root) = Self::respawn_ops(nodes, &parent, &name);
            ops.append(&mut node_ops);
            created.push(new_root);
        }
        if created.is_empty() {
            return;
        }
        self.transact(format!("Paste {} node(s)", created.len()), ops);
        self.selected = created;
    }

    /// Duplicate the selection, **children included**.
    ///
    /// Built out of the ops that already exist rather than a `DuplicateNode`
    /// op: spawn, then set what the original had. One transaction, so it is
    /// still one Ctrl+Z.
    ///
    /// This used to spawn the node and none of its descendants, so duplicating
    /// a rig produced an empty rig — see `respawn_ops`, which both this and
    /// paste now go through.
    fn duplicate_selection(&mut self) {
        let mut ops = Vec::new();
        let mut created = Vec::new();
        for path in self.selected.clone() {
            let nodes = self.subtree(&path);
            let Some(root) = nodes.first() else { continue };
            let Some((parent, name)) = path.rsplit_once('/') else {
                // A root node has nowhere to be a sibling of.
                crate::log::warn(format!("{path} is a root node; nothing to duplicate it into"));
                continue;
            };
            let (parent, name) = (parent.to_owned(), name.to_owned());
            let copy = self.free_name(&parent, &name);
            let (mut node_ops, new_root) = Self::respawn_ops(&nodes, &parent, &copy);
            // Offset so the copy is visible rather than exactly inside the
            // original, which is what the old code did and is why it was here.
            let t = &root.transform;
            node_ops.push(transform_op(
                &new_root,
                Some([t.pos[0] + 1.0, t.pos[1], t.pos[2]]),
                Some(t.rot_euler),
                Some(t.scale),
            ));
            ops.append(&mut node_ops);
            created.push(new_root);
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
        // **Rings before lines, in Rotate mode.** The ring is the thing drawn
        // in that mode, so it is the thing a press should find; the axis lines
        // stay grabbable underneath for anyone who liked them.
        if self.mode == Mode::Rotate
            && let Some(axis) = gizmo::grab_ring(&self.rings, self.cursor)
            && let Some(node) = self.focused()
            && let Some(transform) = self.view.transform_of(&node)
            && let Some(handle) = self.handles.iter().find(|h| h.axis == axis).cloned()
        {
            let group = self
                .selected
                .iter()
                .filter_map(|path| {
                    let t = self.view.transform_of(path)?;
                    Some((path.clone(), [t.pos, t.rot_euler, t.scale]))
                })
                .collect();
            let centre = handle.origin;
            // The world point the ring turns about — the same one the ring was
            // drawn around.
            let world_centre = self
                .subtree_bounds(&node)
                .map_or(Vec3::ZERO, |(min, max)| (min + max) * 0.5);
            self.drag = Some(Drag {
                handle,
                from: self.cursor,
                start: [transform.pos, transform.rot_euler, transform.scale],
                node,
                group,
                centre,
                from_angle: gizmo::angle_about(centre, self.cursor),
                ring: true,
                sign: gizmo::ring_sign(&self.projection(), world_centre, axis),
                also: None,
            });
            return;
        }
        // **A plane quad before the lines it sits between.** It is drawn on
        // top and is the smaller target, so a press inside it means the plane.
        if self.mode == Mode::Move
            && let Some((first, second)) = gizmo::grab_plane(&self.planes, self.cursor)
            && let Some(node) = self.focused()
            && let Some(transform) = self.view.transform_of(&node)
            && let Some(a) = self.handles.iter().find(|h| h.axis == first).cloned()
            && let Some(b) = self.handles.iter().find(|h| h.axis == second).cloned()
        {
            let group = self
                .selected
                .iter()
                .filter_map(|path| {
                    let t = self.view.transform_of(path)?;
                    Some((path.clone(), [t.pos, t.rot_euler, t.scale]))
                })
                .collect();
            self.drag = Some(Drag {
                centre: a.origin,
                handle: a,
                from: self.cursor,
                start: [transform.pos, transform.rot_euler, transform.scale],
                node,
                group,
                from_angle: 0.0,
                ring: false,
                sign: 1.0,
                also: Some(b),
            });
            return;
        }
        if let Some(index) = gizmo::grab(&self.handles, self.cursor) {
            let Some(node) = self.focused() else { return };
            let Some(transform) = self.view.transform_of(&node) else {
                return;
            };
            let group = self
                .selected
                .iter()
                .filter_map(|path| {
                    let t = self.view.transform_of(path)?;
                    Some((path.clone(), [t.pos, t.rot_euler, t.scale]))
                })
                .collect();
            self.drag = Some(Drag {
                handle: self.handles[index].clone(),
                from: self.cursor,
                start: [transform.pos, transform.rot_euler, transform.scale],
                node,
                group,
                centre: self.handles[index].origin,
                from_angle: 0.0,
                ring: false,
                sign: 1.0,
                also: None,
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
        // **Every selected node moves, each from its own start** — ADR 0098.
        // The group falls back to the focused node alone, which is what a
        // single selection is.
        let group = if drag.group.is_empty() {
            vec![(drag.node.clone(), drag.start)]
        } else {
            drag.group.clone()
        };
        // Ctrl inverts the toggle rather than setting it, so somebody working
        // on the grid can step off it for one drag — see `gizmo::Snap`.
        let inverted = self.input.held("ControlLeft") || self.input.held("ControlRight");
        let step = self.snap.step(self.mode, inverted);
        let round = |value: f32| step.map_or(value, |step| gizmo::snap(value, step));

        let mut ops = Vec::with_capacity(group.len());
        for (node, start) in &group {
        let node = node.clone();
        let (start_pos, start_rot, start_scale) = (start[0], start[1], start[2]);
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
                // The plane's second axis, geared by its own handle — ADR 0099.
                if let Some(other) = drag.also.as_ref() {
                    world_delta[other.axis] =
                        gizmo::drag_distance(other, drag.from, self.cursor);
                }
                let local_delta = self
                    .view
                    .parent_inverse(&node)
                    .transform_vector3(world_delta);

                let pos = [
                    round(start_pos[0] + local_delta.x),
                    round(start_pos[1] + local_delta.y),
                    round(start_pos[2] + local_delta.z),
                ];
                (format!("Move {node}"), transform_op(&node, Some(pos), None, None))
            }
            Mode::Rotate => {
                let mut rot = start_rot;
                // **A ring turns by the angle the hand swept about it** — ADR
                // 0099 — and an axis line still turns by how far it was
                // dragged, so the old handles keep working. One drag cannot
                // pass half a turn, because the swept angle is measured
                // absolutely from the press and wraps at pi; that is the price
                // of being drift-free across a dropped frame.
                let degrees = if drag.ring {
                    // Signed by which side of the plane the camera is on, or
                    // the object counter-rotates from behind — see `ring_sign`.
                    gizmo::shortest_turn(
                        drag.from_angle,
                        gizmo::angle_about(drag.centre, self.cursor),
                    )
                    .to_degrees()
                        * drag.sign
                } else {
                    travelled * ROTATE_PER_UNIT
                };
                rot[axis] = round(start_rot[axis] + degrees);
                (
                    format!("Rotate {node}"),
                    transform_op(&node, None, Some(rot), None),
                )
            }
            Mode::Scale => {
                let mut scale = start_scale;
                // Additive, not multiplicative: a scale of zero would otherwise
                // be a trap you cannot drag back out of.
                scale[axis] = round(start_scale[axis] + travelled).max(0.01);
                (
                    format!("Scale {node}"),
                    transform_op(&node, None, None, Some(scale)),
                )
            }
        };
        let _ = label;
        ops.push(op);
        }

        // **One gesture, one transaction, one undo step** — whether it moved
        // one node or sixty. The gesture key is the focused node's, so a drag
        // coalesces with itself and not with the next one.
        let what = match group.len() {
            1 => format!("{} {}", self.mode.label(), group[0].0),
            n => format!("{} {n} nodes", self.mode.label()),
        };
        let gesture = format!("gizmo:{node}:{axis}:{}", self.gesture_epoch);
        self.transact_as(what, ops, Some(&gesture));
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
        // **Not while playing** — ADR 0099. `view.picks` holds the *authored*
        // scene's boxes and the viewport is drawing the simulated world, so a
        // click on the boat where it has sailed to selects whatever used to be
        // there, or nothing. Selecting from the hierarchy still works, which is
        // what tuning needs; a viewport pick that answers about a world nobody
        // is looking at is worse than no pick.
        if self.play.is_some() {
            return;
        }
        let projection = self.projection();
        let dir = projection.ray(self.cursor.0, self.cursor.1);

        // **Every hit along the ray, nearest first** — ADR 0099. One nearest
        // box is wrong the moment two things overlap, which in a 260-node scene
        // is most of the screen: a crate inside a hold, a lamp against a wall,
        // a mesh leaf inside the character that owns it. Picking is a ray
        // against AABBs (`ponytail:` above), so the *nearest* answer is often
        // the box that merely encloses what you meant.
        let mut hits: Vec<(f32, &String)> = self
            .view
            .picks
            .iter()
            .filter_map(|(path, bounds)| {
                ray_box(projection.eye(), dir, bounds).map(|t| (t, path))
            })
            .collect();
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));

        // **Clicking the same spot again takes the next one down.** That is how
        // you reach the thing inside the box without hunting the hierarchy, and
        // it is the cheap half of pixel-accurate picking: the expensive half is
        // an ID buffer and a readback, and this removes most of the reason to
        // want one.
        let same_spot = self
            .last_pick
            .is_some_and(|(x, y)| (x - self.cursor.0).abs() < 3.0 && (y - self.cursor.1).abs() < 3.0);
        let start = if same_spot {
            hits.iter()
                .position(|(_, path)| self.selected.first() == Some(*path))
                .map_or(0, |at| (at + 1) % hits.len().max(1))
        } else {
            0
        };
        self.last_pick = Some(self.cursor);
        let best = hits.get(start).or_else(|| hits.first()).copied();
        if hits.len() > 1 && same_spot {
            crate::log::info(format!("{} of {} under the cursor", start + 1, hits.len()));
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



    /// The prefabs this scene declares, and who instances them — ADR 0101.
    ///
    /// **From the session's text, not `view.scene`.** The view holds the
    /// *resolved* scene: `prefab_load::for_reading` has already replaced every
    /// instance with the subtree it stood for, so it declares no prefabs and
    /// has no nodes carrying a `prefab` key. Asking it produced "this scene
    /// declares no prefabs" for a scene that declares five.
    ///
    /// Same reason the override markers parse the session text — resolution
    /// erases exactly what these two panels are about.
    fn recompute_prefabs(&mut self) {
        let Some(text) = self.session.as_ref().map(loom_scene::Session::text) else {
            self.prefabs = Vec::new();
            return;
        };
        let Ok(unresolved) = loom_scene::Scene::parse(text) else {
            return;
        };
        self.prefabs = unresolved
            .prefabs()
            .into_iter()
            .map(|decl| loom_editor::PrefabRow {
                instances: unresolved
                    .nodes()
                    .iter()
                    .filter(|node| node.prefab.as_deref() == Some(decl.key.as_str()))
                    .map(|node| node.path.clone())
                    .collect(),
                key: decl.key,
                path: decl.path,
                id: decl.id,
            })
            .collect();
    }

    /// What is wrong with the scene, for the Problems panel — ADR 0093.
    ///
    /// **The same three sources `loom validate` reads**, so the panel and the
    /// command cannot disagree: an asset alias that resolves to nothing, a
    /// voxel op list that will not parse, and a prefab override pointing at a
    /// child that no longer exists.
    ///
    /// Recomputed when the scene changes rather than every frame. A scene of
    /// 260 nodes is cheap to walk once and wasteful to walk at 144 Hz.
    fn recompute_problems(&mut self) {
        let mut problems = Vec::new();
        let base = self
            .scene_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();

        let (errors, warnings) = crate::alias_report(&self.view.scene, &base);
        let field = |value: &serde_json::Value, key: &str| {
            value.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned()
        };
        for (list, blocking) in [(errors, true), (warnings, false)] {
            for entry in list {
                let message = match (field(&entry, "constraint"), field(&entry, "error")) {
                    (c, e) if c.is_empty() => e,
                    (c, _) => c,
                };
                problems.push(loom_editor::Problem {
                    blocking,
                    node: field(&entry, "node"),
                    message,
                });
            }
        }

        for node in self.view.scene.nodes() {
            if let Some(component) = node.components.get("VoxelVolume")
                && let Err(e) = crate::parse_ops(component)
            {
                problems.push(loom_editor::Problem {
                    blocking: true,
                    node: node.path.clone(),
                    message: format!("VoxelVolume: {e}"),
                });
            }
        }

        // Overrides that point at nothing. The map is keyed by resolved path,
        // so a key with no node behind it is an override the prefab no longer
        // has a home for — §5 says that is a loud warning, never a silent drop.
        let overrides = self
            .session
            .as_ref()
            .map(|session| crate::override_map(session.text()))
            .unwrap_or_default();
        for path in overrides.keys() {
            if !self.view.paths.iter().any(|p| p == path) {
                problems.push(loom_editor::Problem {
                    blocking: false,
                    node: path.clone(),
                    message: "override targets a node that is not in the scene".to_owned(),
                });
            }
        }

        problems.sort_by(|a, b| b.blocking.cmp(&a.blocking).then(a.node.cmp(&b.node)));
        self.problems = problems;
    }



    /// Carry an edit made during Play into the running game — ADR 0099.
    ///
    /// **Rebuild and restore, rather than patch the world.** Everything derived
    /// is cached when Play starts: `Sim::new` walks the world once and keeps
    /// `characters`, `floating` and `propelled`, and the weather stages live on
    /// the runner. Writing a changed field into the live world would therefore
    /// do nothing for almost every component — a knob that moves and changes
    /// not one thing, which is worse than a knob that is greyed out.
    ///
    /// So the edit goes to the scene the normal way, Play is rebuilt from the
    /// edited scene, and the simulation is put back with the ADR 0088 snapshot —
    /// the same save/restore the CLI uses and the gate proves byte-exact. The
    /// boat keeps its position, velocity, wake, thrust and script memory, and
    /// starts obeying the new number on the next tick.
    ///
    /// **A transform edit is the one that will not stick**, because the snapshot
    /// restores where things *were*: move a node during Play and the running
    /// world puts it back. Tuning a parameter is the case this exists for.
    fn reapply_to_play(&mut self) {
        let Some(old) = self.play.as_ref() else {
            return;
        };
        let ticks = old.ticks;
        let snapshot = old.save_game(u64::from(ticks));

        let world = loom_ecs::World::from_scene(&self.view.scene);
        let mut play = crate::play::Play::start(world, &self.base);
        play.load_game(&snapshot);
        play.ticks = ticks;
        self.play = Some(play);
        self.refresh_play_objects();
    }



    /// Spawn a node with `alias` where the cursor let go — ADR 0099.
    ///
    /// **Placed where the ray lands, not at the origin.** Dropping a crate and
    /// finding it at the world origin is not placement, it is a spawn button
    /// with extra steps. The ray goes through whatever is already in the scene;
    /// failing that it lands a few metres out, which is what an empty view
    /// deserves.
    fn drop_asset(&mut self, alias: &str, at: (f32, f32)) {
        if self.session.is_none() || self.play.is_some() {
            return;
        }
        let projection = self.projection();
        let dir = projection.ray(at.0, at.1);
        let eye = projection.eye();

        let hit = self
            .view
            .picks
            .values()
            .filter_map(|bounds| ray_box(eye, dir, bounds))
            .min_by(f32::total_cmp);
        let point = eye + dir * hit.unwrap_or(8.0);

        // Under the selection when there is one, so dropping into a rig keeps
        // it in the rig — the same rule paste follows.
        let parent = self.selected.first().cloned().unwrap_or_default();
        let name = self.free_name(&parent, alias);
        let path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        let local = self
            .view
            .parent_inverse(&path)
            .transform_point3(point)
            .to_array();

        self.transact(
            format!("Drop {alias}"),
            vec![
                loom_scene::SceneOp::SpawnNode {
                    parent,
                    name,
                    mesh: Some(alias.to_owned()),
                    prefab: None,
                },
                transform_op(&path, Some(local), None, None),
            ],
        );
        self.selected = vec![path];
    }

    /// A scene id derived from where the file is — ADR 0099.
    ///
    /// **Not random.** This project has no uuid dependency and does not want
    /// one for a string, and `Math::random` is banned from the determinism
    /// path anyway. A hash of the path formatted as a uuid is stable, unique
    /// per file, and readable as what it is; two scenes cannot collide unless
    /// they are the same file.
    fn scene_id_for(path: &std::path::Path) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        let a = hash;
        let b = hash.rotate_left(17).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            (a >> 32) as u32,
            (a >> 16) as u16,
            (a & 0x0fff) as u16,
            (b >> 52) as u16 & 0x0fff,
            b & 0xffff_ffff_ffff,
        )
    }

    /// The smallest thing `Scene::parse` accepts and the viewer can show: one
    /// root node, and an id because §3 wants one.
    ///
    /// **Shared with its test**, which used to re-declare the string — so the
    /// test proved a copy parsed while the template drifted underneath it.
    fn blank_scene(id: &str) -> String {
        format!("# A new scene.\n[scene]\nformat = 1\nid = \"{id}\"\n\n[[node]]\nname = \"Root\"\n")
    }

    /// Start an empty scene beside this one — ADR 0099.
    ///
    /// **A real file, immediately.** An unsaved in-memory document would need a
    /// second notion of "where does this live" everywhere the editor already
    /// answers that with `scene_path`, and the first Save would need the dialog
    /// this project does not have.
    fn new_scene(&mut self) {
        let mut path = self.base.join("untitled.loom");
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = self.base.join(format!("untitled{n}.loom"));
        }
        let scene = Self::blank_scene(&Self::scene_id_for(&path));
        if let Err(e) = std::fs::write(&path, scene) {
            crate::log::error(format!("{}: {e}", path.display()));
            return;
        }
        crate::log::info(format!("created {}", path.display()));
        let path = path.to_string_lossy().into_owned();
        self.open_scene(&path);
    }

    /// Write the scene under another name, beside this one — ADR 0099.
    ///
    /// The new file becomes the open one, which is what "save as" means
    /// everywhere; a copy you are not editing is a different verb.
    fn save_as(&mut self, name: &str) {
        let name = if name.ends_with(".loom") {
            name.to_owned()
        } else {
            format!("{name}.loom")
        };
        let path = self.base.join(&name);
        if path.exists() {
            crate::log::warn(format!("{} already exists; pick another name", path.display()));
            return;
        }
        let Some(session) = self.session.as_ref() else {
            crate::log::warn("this scene is open read-only".to_owned());
            return;
        };
        // **A copy needs its own identity.** Writing the session verbatim kept
        // the original's `[scene] id`, so the two files claimed to be the same
        // scene — the exact collision `scene_id_for` exists to prevent, created
        // by the one command whose whole job is to make a second file.
        let text = replace_scene_id(session.text(), &Self::scene_id_for(&path));
        if let Err(e) = std::fs::write(&path, text) {
            crate::log::error(format!("{}: {e}", path.display()));
            return;
        }
        crate::log::info(format!("saved as {}", path.display()));
        let path = path.to_string_lossy().into_owned();
        // The copy on disk is what the session already held, so nothing is
        // unsaved and `open_scene` will not refuse.
        self.dirty = false;
        self.open_scene(&path);
    }



    /// Ask the agent for something — ADR 0100.
    ///
    /// **The selection travels with the request.** "Make it sit lower" is not a
    /// sentence about anything until it carries what was selected when it was
    /// typed, and an agent reading the inbox has no other way to know.
    fn ask_agent(&mut self, text: &str) {
        match crate::agent_link::ask(&self.scene_path, text, &self.selected) {
            Ok(id) => {
                crate::log::info(format!("asked the agent (#{id}): {text}"));
                self.refresh_agent_chat();
            }
            Err(e) => crate::log::error(format!("could not reach the agent: {e}")),
        }
    }

    /// Re-read the conversation from disk — ADR 0100.
    ///
    /// Polled on the same tick as the scene file, because a reply and the edit
    /// it describes arrive together and reading one without the other shows a
    /// changed scene nobody explained.
    fn refresh_agent_chat(&mut self) {
        let turns = crate::agent_link::transcript(&self.scene_path);
        self.agent_busy = !crate::agent_link::pending(&self.scene_path).is_empty();
        self.agent_chat = turns
            .into_iter()
            .map(|m| loom_editor::AgentTurn {
                from_agent: m.speaker == crate::agent_link::Speaker::Agent,
                text: m.text,
                about: m.about,
            })
            .collect();
    }

    /// Where a game saved from the editor lands — ADR 0098.
    ///
    /// **Beside the scene, named after it.** No file dialog: this project has
    /// no dependency that draws one, and inventing a path scheme a human cannot
    /// guess would be worse than one they can read off the console line.
    fn save_game_path(&self) -> std::path::PathBuf {
        self.scene_path.with_extension("save.json")
    }

    /// Write the running game out — ADR 0098.
    fn save_game(&mut self) {
        let path = self.save_game_path();
        let Some(play) = self.play.as_ref() else {
            crate::log::warn("nothing is playing".to_owned());
            return;
        };
        let save = play.save_game(u64::from(play.ticks));
        match serde_json::to_string_pretty(&save)
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()))
        {
            Ok(()) => crate::log::info(format!("saved the game to {}", path.display())),
            Err(e) => crate::log::error(format!("{}: {e}", path.display())),
        }
    }

    /// Read one back into the running game — ADR 0098.
    fn load_game(&mut self) {
        let path = self.save_game_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                crate::log::error(format!("{}: {e}", path.display()));
                return;
            }
        };
        let Ok(save) = serde_json::from_str::<serde_json::Value>(&text) else {
            crate::log::error(format!("{} is not a save file", path.display()));
            return;
        };
        let Some(play) = self.play.as_mut() else {
            crate::log::warn("start playing before loading a game".to_owned());
            return;
        };
        play.load_game(&save);
        crate::log::info(format!("loaded the game from {}", path.display()));
    }


    /// The union of a node's own bounds and every descendant's — ADR 0099.
    ///
    /// **A rig node has no mesh.** `Rig/Boat` is the whole boat and carries no
    /// `MeshRenderer`, so anything keyed on the node's own bounds — the gizmo,
    /// the selection box, the orbit pivot — had nothing to work with for
    /// exactly the nodes a human clicks first. The gizmo simply did not appear.
    fn subtree_bounds(&self, path: &str) -> Option<(Vec3, Vec3)> {
        let prefix = format!("{path}/");
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        let mut found = false;
        for (candidate, bounds) in &self.view.picks {
            if candidate != path && !candidate.starts_with(&prefix) {
                continue;
            }
            found = true;
            for axis in 0..3 {
                min[axis] = min[axis].min(bounds.min[axis]);
                max[axis] = max[axis].max(bounds.max[axis]);
            }
        }
        found.then(|| (Vec3::from_array(min), Vec3::from_array(max)))
    }

    /// The selection's bounding box as screen-space edges — ADR 0099.
    ///
    /// **Twelve segments per selected node**, projected here because the
    /// projection belongs to the camera and `loom_editor` is handed only what
    /// it draws. A corner behind the eye drops its segments rather than
    /// projecting to a wild coordinate and drawing a line across the window.
    fn selection_edges(&self, projection: &gizmo::View) -> Vec<loom_editor::panels::Segment> {
        // The twelve edges of a box, as pairs of corner indices, where a
        // corner's bits are (x, y, z) taken from min or max.
        const EDGES: [(usize, usize); 12] = [
            (0, 1), (0, 2), (0, 4), (1, 3), (1, 5), (2, 3),
            (2, 6), (3, 7), (4, 5), (4, 6), (5, 7), (6, 7),
        ];
        // Same reason the pick is disabled while playing: these boxes are the
        // authored scene's, and drawing them over the simulated world puts them
        // where things *were*.
        if self.play.is_some() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for path in &self.selected {
            // The subtree's bounds, not the node's own — see `subtree_bounds`.
            let Some((min, max)) = self.subtree_bounds(path) else {
                continue;
            };
            let (min, max) = (min.to_array(), max.to_array());
            let corners: Vec<Option<(f32, f32)>> = (0..8)
                .map(|i| {
                    let pick = |bit: usize, axis: usize| {
                        if i & (1 << bit) == 0 { min[axis] } else { max[axis] }
                    };
                    projection.project(loom_render::glam::Vec3::new(
                        pick(0, 0),
                        pick(1, 1),
                        pick(2, 2),
                    ))
                })
                .collect();
            for (a, b) in EDGES {
                if let (Some(from), Some(to)) = (corners[a], corners[b]) {
                    out.push((from, to));
                }
            }
        }
        out
    }

    /// What the camera dollies toward and orbits around — ADR 0098.
    ///
    /// **The selection when there is one**, because that is what a human is
    /// working on and what they expect to stay in frame. Failing that, a point
    /// a few metres ahead, so scrolling in open space still behaves.
    fn orbit_pivot(&self) -> Vec3 {
        // While playing the authored bounds are stale, so the camera turns
        // about a point ahead of it rather than about where a thing used to be.
        self.selected
            .first()
            .filter(|_| self.play.is_none())
            .and_then(|path| self.subtree_bounds(path))
            .map_or_else(
                || self.camera.position + self.camera.forward() * 8.0,
                |(min, max)| (min + max) * 0.5,
            )
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
        let (copy, paste) = (act("copy"), act("paste"));
        let (select_all, deselect) = (act("select_all"), act("deselect"));
        let rename_here = act("rename");
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
        if select_all {
            self.selected = self.view.paths.clone();
        }
        if deselect {
            // **Emptied, not reset to the first node.** "Nothing is selected"
            // is a state a human asks for — it is how you stop the gizmo
            // drawing over the thing you are looking at.
            self.selected.clear();
        }
        if rename_here {
            self.renaming = self.selected.first().cloned();
        }
        if copy {
            self.copy_selection();
        }
        if paste {
            self.paste_clipboard();
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
/// The control scheme, in three layers — ADR 0086.
///
/// **Engine, then game, then player, each replacing the last per action.** It
/// used to be one fixed path, so a scene could not ship its own scheme without
/// replacing everyone else's, and a player could not rebind anything without
/// editing the file the game shipped.
///
/// A layer that fails to load is reported and skipped rather than fatal: a
/// player with a typo in their bindings should get the game's controls and a
/// message, not a window that will not open.
fn load_bindings(view: &SceneView) -> ActionMap {
    let mut map = ActionMap::from_toml(loom_input::DEFAULT_BINDINGS).unwrap_or_default();
    let mut overlay_from = |path: &std::path::Path, what: &str| {
        if !path.exists() {
            return;
        }
        match ActionMap::load(path) {
            Ok(other) => map.overlay(other),
            Err(e) => eprintln!("loom: {} ({what}): {e}; ignored", path.display()),
        }
    };
    // The engine's project-local file, kept for every scene that names nothing.
    overlay_from(std::path::Path::new("assets/input/default.toml"), "defaults");
    // The scene's own scheme.
    if let Some(scene_path) = view.world().bindings_path() {
        overlay_from(std::path::Path::new(scene_path), "scene");
    }
    // The player's, last, so it wins.
    if let Some(user) = user_bindings_path() {
        overlay_from(&user, "yours");
    }
    map
}

/// Where a player's own bindings live.
///
/// `$XDG_CONFIG_HOME/loom/bindings.toml`, falling back to `~/.config`. Absent is
/// the normal case and means "no rebindings", not an error.
fn user_bindings_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("loom").join("bindings.toml"))
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
    script: Script,
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
    let scripted = script.driving();
    // **An asked-for selection, applied where the script lands** — ADR 0099.
    // The view is built before `App` exists and `show` only runs on a *change*,
    // so this is the one point that sees both the opening scene and the flag.
    if let Some(wanted) = script.select.clone()
        && app.view.paths.contains(&wanted)
    {
        app.selected = vec![wanted];
    }
    // **Derived state, once, for the scene the editor opens on.** Both of these
    // live in `show`, which runs when the scene *changes* — so on a freshly
    // opened editor the Problems panel said "nothing to report" about a scene
    // `loom validate` rejects, and the Project panel listed no scenes at all,
    // until the human made an unrelated edit. Same shape as the `--select`
    // flag: the first view is built before this struct exists.
    app.recompute_problems();
    app.recompute_prefabs();
    app.scenes = app.sibling_scenes();
    app.refresh_agent_chat();
    if let Some(wanted) = script.mode.as_deref() {
        app.mode = match wanted.to_ascii_lowercase().as_str() {
            "rotate" => Mode::Rotate,
            "scale" => Mode::Scale,
            _ => Mode::Move,
        };
    }
    app.script = script;
    // A front end only makes sense in front of a game, so `--play` is what
    // arms it — and when it is armed it takes autoplay's job: the game starts
    // when the human asks for it, under the black. `session` is `Some` for
    // exactly the runs that got `--edit`; see [`front_end_wanted`] for why that
    // is in the condition and what happened while it was not.
    // A script cannot click Start, so a scripted run skips the front end
    // entirely and falls into the autoplay path below — see [`Script::driving`].
    app.front = front_end_wanted(autoplay && !scripted, app.session.is_some())
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
/// Swap a scene's `id` for another, leaving the rest of the file alone.
///
/// Text, not a re-serialise: the whole point of the format's op layer is that a
/// human's comments and spacing survive an edit, and "save as" must not be the
/// one command that reformats their file on the way out.
fn replace_scene_id(text: &str, id: &str) -> String {
    let mut out = Vec::new();
    let mut in_scene = false;
    let mut written = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            // Leaving `[scene]` without having seen an id: add one, so a file
            // that never had one still gets its own.
            if in_scene && !written {
                out.push(format!("id = \"{id}\""));
                written = true;
            }
            in_scene = trimmed.starts_with("[scene]");
        }
        if in_scene && !written && trimmed.starts_with("id") && trimmed.contains('=') {
            out.push(format!("id = \"{id}\""));
            written = true;
            continue;
        }
        out.push(line.to_owned());
    }
    if in_scene && !written {
        out.push(format!("id = \"{id}\""));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

pub fn open_scene(
    path: &str,
    editable: bool,
    frames: Option<u32>,
    autoplay: bool,
    script: Script,
) -> Result<(), String> {
    let src = loom_asset::pack::read_text(std::path::Path::new(path))
        .map_err(|e| format!("{path}: {e}"))?;
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

    run(path, view, session, disk_seen, frames, autoplay, script)
}

#[cfg(test)]
mod tests {

    /// **A saved copy is a different scene.** `save_as` wrote the session
    /// verbatim, so the duplicate kept the original's id and the two files
    /// claimed to be the same scene — created by the one command whose whole
    /// job is to make a second file.
    #[test]
    fn saving_as_gives_the_copy_its_own_id() {
        let original = "[scene]\nformat = 1\nid = \"aaaaaaaa-0000-4000-8000-000000000000\"\n\n\
             # a comment the human wrote\n[[node]]\nname = \"Root\"\n";
        let copy = super::replace_scene_id(original, "bbbbbbbb-1111-4111-8111-111111111111");
        assert!(copy.contains("bbbbbbbb-1111-4111-8111-111111111111"), "{copy}");
        assert!(!copy.contains("aaaaaaaa"), "the old id must be gone: {copy}");
        assert!(
            copy.contains("# a comment the human wrote"),
            "the rest of the file survives: {copy}"
        );
        loom_scene::Scene::parse(&copy).expect("still a scene");
    }

    /// A scene with no id gets one rather than silently staying anonymous.
    #[test]
    fn a_scene_without_an_id_gains_one() {
        let original = "[scene]\nformat = 1\n\n[[node]]\nname = \"Root\"\n";
        let copy = super::replace_scene_id(original, "cccccccc-2222-4222-8222-222222222222");
        assert!(copy.contains("cccccccc-2222"), "{copy}");
        loom_scene::Scene::parse(&copy).expect("still a scene");
    }

    /// **Only the scene's id.** A node called `id` or an asset id elsewhere in
    /// the file must not be rewritten by a text pass.
    #[test]
    fn only_the_scene_table_is_touched() {
        let original = "[scene]\nformat = 1\nid = \"aaaaaaaa-0000-4000-8000-000000000000\"\n\n\
             [[prefab]]\nkey = \"crate\"\nid = \"7a41c0de-5b2e-4f18-9d63-2c8ae5f10b47\"\n\
             path = \"../p/crate.loom\"\n";
        let copy = super::replace_scene_id(original, "dddddddd-3333-4333-8333-333333333333");
        assert!(copy.contains("dddddddd-3333"), "{copy}");
        assert!(
            copy.contains("7a41c0de-5b2e-4f18-9d63-2c8ae5f10b47"),
            "the prefab's id is not the scene's: {copy}"
        );
    }


    /// **A new scene has to parse.** It is written straight to disk and opened,
    /// so a template with a malformed id or a missing field would create a file
    /// the editor then refuses to load — and the human is left with an
    /// untitled.loom they did not ask for and cannot open.
    #[test]
    fn a_new_scene_is_a_scene() {
        let path = std::path::Path::new("/tmp/loom-new-scene-test/untitled.loom");
        let id = super::App::scene_id_for(path);
        // The template itself, not a copy of it.
        let text = super::App::blank_scene(&id);
        let scene = loom_scene::Scene::parse(&text).expect("the template must parse");
        assert_eq!(scene.nodes().len(), 1);
        assert_eq!(scene.scene_id().as_deref(), Some(id.as_str()));
    }

    /// The id is shaped like a uuid and is stable for a path — two scenes
    /// cannot collide unless they are the same file.
    #[test]
    fn a_scene_id_is_uuid_shaped_and_stable() {
        let a = super::App::scene_id_for(std::path::Path::new("/a/one.loom"));
        let b = super::App::scene_id_for(std::path::Path::new("/a/two.loom"));
        assert_ne!(a, b, "different files, different ids");
        assert_eq!(a, super::App::scene_id_for(std::path::Path::new("/a/one.loom")));
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.len(), 5, "{a}");
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{a}"
        );
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'), "{a}");
    }

    /// **Orbit keeps the pivot where it is and the eye the same distance from
    /// it.** That is the whole contract; a version that drifted would slowly
    /// lose the thing you were circling, which is exactly the failure a fly
    /// camera already has.
    #[test]
    fn orbiting_holds_the_pivot_and_the_radius() {
        let pivot = super::Vec3::new(3.0, 1.0, -2.0);
        let mut camera = super::FlyCamera {
            position: pivot + super::Vec3::new(0.0, 0.0, 10.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_y_degrees: 60.0,
        };
        let before = (camera.position - pivot).length();
        for _ in 0..24 {
            camera.orbit(pivot, 0.25, 0.05);
        }
        let after = (camera.position - pivot).length();
        assert!((after - before).abs() < 1e-3, "radius drifted: {before} -> {after}");
        // And it is still looking at the thing it is going round.
        let to_pivot = (pivot - camera.position).normalize();
        assert!(
            camera.forward().dot(to_pivot) > 0.999,
            "the camera stopped facing the pivot"
        );
    }

    /// Pitch is clamped, or orbiting past vertical flips the world over.
    #[test]
    fn orbiting_cannot_go_over_the_pole() {
        let pivot = super::Vec3::ZERO;
        let mut camera = super::FlyCamera {
            position: super::Vec3::new(0.0, 0.0, 5.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_y_degrees: 60.0,
        };
        for _ in 0..200 {
            camera.orbit(pivot, 0.0, 0.1);
        }
        assert!(camera.pitch <= 1.53, "pitch ran past the pole: {}", camera.pitch);
        assert!(camera.position.is_finite(), "{:?}", camera.position);
    }

    /// **A notch has to mean something at both ends.** Scrolling toward a
    /// doorknob and toward a harbour are the same gesture, so the step scales
    /// with how far away the pivot is.
    #[test]
    fn a_dolly_notch_scales_with_distance() {
        let make = || super::FlyCamera {
            position: super::Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            fov_y_degrees: 60.0,
        };
        let (mut near, mut far) = (make(), make());
        near.dolly(1.0, 2.0);
        far.dolly(1.0, 200.0);
        assert!(
            far.position.length() > near.position.length() * 5.0,
            "near {} vs far {}",
            near.position.length(),
            far.position.length()
        );
        // And it is bounded, so a pivot a kilometre off does not teleport you.
        let mut absurd = make();
        absurd.dolly(1.0, 100_000.0);
        assert!(absurd.position.length() <= 40.0, "{}", absurd.position.length());
    }

    /// Panning moves across the view, never along it.
    #[test]
    fn panning_stays_in_the_view_plane() {
        let mut camera = super::FlyCamera {
            position: super::Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.3,
            fov_y_degrees: 60.0,
        };
        let forward = camera.forward();
        camera.pan(40.0, 25.0, 10.0);
        let moved = camera.position;
        assert!(moved.length() > 0.0, "pan did nothing");
        assert!(
            moved.normalize().dot(forward).abs() < 1e-3,
            "pan drifted along the view direction"
        );
    }

    use super::{
        CODE_ALPHABET, CODE_DENY, CODE_SYMBOLS, Escape, egui, encode, escape_means, format_code,
        front_end_wanted, generate_code, normalise, room_code_panel, title_answers, wholesome,
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
                let _ = crate::hud::pause_menu(
                    root,
                    crate::hud::MenuNav::default(),
                    &mut crate::hud::MenuFocus::default(),
                );
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

    /// **A title screen belongs in front of a game and nowhere else**, and
    /// `--play` alone does not say that: `--edit --play` is legal, and it used
    /// to open a title over the editor's docks. Escape there is `Close`, and
    /// `front` is only ever cleared on the way *out* of the curtain — so a
    /// human who reached for the editor's own Play button afterwards would find
    /// the pause menu unreachable behind a title that never leaves.
    #[test]
    fn a_front_end_goes_in_front_of_a_game_and_never_in_front_of_the_editor() {
        // (autoplay, editing)
        for (state, want) in [
            ((true, false), true),   // `--play`: the demo, and the whole point
            ((true, true), false),   // `--edit --play`: the editor wins
            ((false, true), false),  // `--edit`: no game to be in front of
            ((false, false), false), // the read-only viewer
        ] {
            let (autoplay, editing) = state;
            assert_eq!(
                front_end_wanted(autoplay, editing),
                want,
                "front_end_wanted{state:?}"
            );
        }
    }

    /// **The menu answers exactly when the player can see it**, which is the
    /// property, not a duration.
    ///
    /// `hud::fade` paints and a painter takes no input, so both buttons are
    /// live from the first frame — under an opening curtain that starts at 99%
    /// black. Equality in both directions is what makes this falsifiable:
    /// dropping the guard makes the early samples answer while the curtain is
    /// still up, and gating on anything longer than the curtain leaves a dead
    /// button on a picture the player is looking at.
    #[test]
    fn the_title_answers_a_click_exactly_when_the_player_can_see_the_button() {
        // Where a fresh `Front` starts: the top of the fade-*up*.
        let opening = crate::hud::FADE_OUT + crate::hud::HOLD;
        let mut answered_while_black = 0;
        for i in 0..=150u16 {
            let clock = f32::from(i).mul_add(crate::hud::FADE_IN / 100.0, opening);
            let hidden = crate::hud::curtain(clock) > 0.0;
            assert_eq!(
                title_answers(clock),
                !hidden,
                "{clock:.3}s in, the curtain is {:.3} and the menu answers {}",
                crate::hud::curtain(clock),
                title_answers(clock)
            );
            answered_while_black += usize::from(hidden && title_answers(clock));
        }
        assert_eq!(answered_while_black, 0);
        assert!(
            !title_answers(opening),
            "the very first frame answered a click it had hidden"
        );
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

        // **And what it says, which is the same kind of silent loss.** The
        // word and both control lines are `Hud` elements with `only_on_title`,
        // so nothing else in this project draws them and nothing else fails if
        // they go. A title screen that has quietly lost its controls is a demo
        // nobody can work out how to operate — the failure mode a front end
        // exists to prevent.
        let title = crate::hud::elements(&world, &loom_script::GameState::default(), false, true);
        let lines: Vec<&str> = title.iter().map(crate::hud::Element::text).collect();
        assert!(
            lines.contains(&"DEEPER"),
            "the demo lost its name from the title screen: {lines:?}"
        );
        for key in ["WASD", "MOUSE", "CLICK", "SHIFT", "E ", "TAB"] {
            assert!(
                lines.iter().any(|l| l.contains(key)),
                "the title screen no longer says what {key:?} does: {lines:?}"
            );
        }
    }
}
