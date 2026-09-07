//! Rebindable actions, loaded from TOML.
//!
//! Game code asks "is `move_forward` held?", never "is W down?". That
//! indirection is the whole feature: rebinding is editing a text file, and the
//! bindings are diffable and reviewable like everything else authored here.
//!
//! Contexts exist because the same key means different things in different
//! modes — W flies the camera while flying and does nothing while a menu is
//! open. Without contexts that becomes a pile of `if !menu_open` checks spread
//! through unrelated code.
//!
//! This crate knows nothing about `winit`: it takes opaque button names as
//! strings. That keeps the windowing dependency out of the input layer, and
//! means a headless test can drive input without a window.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

/// Whether an action fires on the transition or while held.
///
/// The distinction matters: "jump" on press should fire once no matter how
/// long the key is down, while "move forward" should apply every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// True on the frame the button went down.
    Pressed,
    /// True for every frame the button is down.
    #[default]
    Held,
    /// True on the frame the button came up.
    Released,
}

/// One way to fire an action.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Binding {
    /// Platform button name, e.g. `KeyW`, `ShiftLeft`, `MouseRight`.
    pub button: String,
    /// Buttons that must also be held. An empty list means "don't care",
    /// *not* "no modifiers" — see [`ActionMap::is_active`].
    #[serde(default)]
    pub modifiers: Vec<String>,
    #[serde(default)]
    pub trigger: Trigger,
}

/// One analog source for an action — a stick or a trigger.
///
/// **Separate from [`Binding`] because a stick is not a button.** A button is
/// down or it is not; a stick reports -1..1 and a trigger 0..1, and squashing
/// either into a boolean at the driver throws away the half of the signal that
/// makes a controller worth having. Walking slowly is the feature.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AxisBinding {
    /// Analog source name, e.g. `PadLeftStickY`, `PadRightTrigger2`.
    pub axis: String,
    /// Multiplier, applied before the dead zone. `-1.0` inverts — which is not
    /// a preference but a fact about the hardware: a stick pushed forward
    /// reports **negative** Y, and every scheme that wants "forward is
    /// positive" says so here rather than in game code.
    #[serde(default = "one")]
    pub scale: f32,
    /// Deflection below which the axis reads zero. A stick at rest is never
    /// exactly zero, and without this a character drifts across the room while
    /// nobody is holding anything.
    #[serde(default = "default_dead_zone")]
    pub dead_zone: f32,
}

fn one() -> f32 {
    1.0
}

fn default_dead_zone() -> f32 {
    // Measured on the pads this was written against: at rest they wander to
    // about 0.08, and 0.15 clears that without eating usable travel.
    0.15
}

/// A named set of actions that are live together.
// **No `Eq`**: an `AxisBinding` holds an `f32`, and a dead zone is a
// measurement rather than an identity. `PartialEq` is all any caller needed.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Context {
    /// Action name → the ways it can fire.
    #[serde(default)]
    pub actions: BTreeMap<String, Vec<Binding>>,
    /// Action name → analog sources. Absent for every context authored before
    /// gamepads existed, which is why it defaults rather than being required.
    #[serde(default)]
    pub axes: BTreeMap<String, Vec<AxisBinding>>,
}

/// Every context, as loaded from TOML.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ActionMap {
    #[serde(default)]
    contexts: BTreeMap<String, Context>,
}

/// Why a binding file could not be used.
#[derive(Debug)]
pub enum InputError {
    Io(std::io::Error),
    /// The file is not valid TOML, or does not describe contexts.
    Parse(String),
    /// A binding names a context or action that cannot work.
    Invalid(String),
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Parse(e) => write!(f, "invalid bindings: {e}"),
            Self::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InputError {}

impl ActionMap {
    /// Parse a binding file.
    ///
    /// # Errors
    /// [`InputError::Parse`] if the TOML is malformed, [`InputError::Invalid`]
    /// if a binding is empty — a binding with no button can never fire, and
    /// silently keeping it would make a typo look like a broken key.
    pub fn from_toml(text: &str) -> Result<Self, InputError> {
        let map: Self = toml::from_str(text).map_err(|e| InputError::Parse(e.to_string()))?;
        for (context, entry) in &map.contexts {
            for (action, bindings) in &entry.actions {
                if bindings.is_empty() {
                    return Err(InputError::Invalid(format!(
                        "{context}.{action} has no bindings, so it can never fire"
                    )));
                }
                if let Some(bad) = bindings.iter().find(|b| b.button.is_empty()) {
                    return Err(InputError::Invalid(format!(
                        "{context}.{action} has a binding with no button: {bad:?}"
                    )));
                }
            }
        }
        Ok(map)
    }

    /// Load a binding file from disk.
    ///
    /// # Errors
    /// [`InputError`] if it cannot be read or is invalid.
    pub fn load(path: &std::path::Path) -> Result<Self, InputError> {
        let text = std::fs::read_to_string(path).map_err(InputError::Io)?;
        Self::from_toml(&text)
    }

    /// Context names, sorted.
    /// Layer `other` on top of this map, action by action — ADR 0086.
    ///
    /// **Per action, not per context and not per file.** A player who rebinds
    /// `jump` should not lose every other binding in that context, and a game
    /// that ships its own scheme should not have to restate the engine's camera
    /// controls to keep them. So an action present in `other` replaces that
    /// action outright — all of its bindings, because a partial merge of two
    /// binding *lists* has no sane meaning — and every action `other` is silent
    /// about is left exactly as it was.
    ///
    /// Axes layer the same way and independently: rebinding the stick does not
    /// disturb the keys bound to the same action.
    pub fn overlay(&mut self, other: Self) {
        for (name, ctx) in other.contexts {
            let target = self.contexts.entry(name).or_default();
            for (action, bindings) in ctx.actions {
                target.actions.insert(action, bindings);
            }
            for (action, axes) in ctx.axes {
                target.axes.insert(action, axes);
            }
        }
    }

    pub fn context_names(&self) -> impl Iterator<Item = &str> {
        self.contexts.keys().map(String::as_str)
    }

    /// Every action in a context, sorted.
    pub fn actions_in(&self, context: &str) -> impl Iterator<Item = &str> {
        self.contexts
            .get(context)
            .into_iter()
            .flat_map(|c| c.actions.keys().map(String::as_str))
    }

    /// Whether `action` is firing, given what is currently down.
    ///
    /// `just_changed` holds buttons whose state changed this frame, which is
    /// what makes `Pressed` and `Released` distinguishable from `Held`.
    #[must_use]
    pub fn is_active(
        &self,
        context: &str,
        action: &str,
        held: &BTreeSet<String>,
        just_changed: &BTreeSet<String>,
        pressed_this_frame: &BTreeMap<String, BTreeSet<String>>,
    ) -> bool {
        let Some(bindings) = self.contexts.get(context).and_then(|c| c.actions.get(action)) else {
            return false;
        };

        bindings.iter().any(|binding| {
            let down = held.contains(&binding.button);
            let changed = just_changed.contains(&binding.button);

            // Modifiers are a requirement, never an exclusion: binding W with
            // no modifiers still fires while Shift is held, because sprinting
            // forward is still moving forward. A binding that needed exclusion
            // would say so, and none has yet.
            match binding.trigger {
                Trigger::Held => {
                    binding.modifiers.iter().all(|m| held.contains(m)) && down
                }
                // A chord counts when its modifiers were held **while the key
                // was down** — which is either at the press itself, or added
                // afterwards while the key is still held. Both are how people
                // actually type a chord.
                //
                // What it must reject is a key that was tapped and released
                // and *then* had a modifier arrive later in the same frame.
                // Latching the press without also pinning when its modifiers
                // applied let that fire the chord: Ctrl+S saving a scene
                // because you reached for Ctrl just after tapping S.
                Trigger::Pressed => pressed_this_frame
                    .get(&binding.button)
                    .is_some_and(|at_press| {
                        binding
                            .modifiers
                            .iter()
                            .all(|m| at_press.contains(m) || (down && held.contains(m)))
                    }),
                Trigger::Released => {
                    binding.modifiers.iter().all(|m| held.contains(m)) && !down && changed
                }
            }
        })
    }
}

/// Tracks which buttons are down, and which changed this frame.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    /// Analog source name → its current deflection. Unlike the button sets this
    /// is *level*, not transition: a stick held still still reports its value,
    /// and clearing it each frame would make a held stick stutter.
    analog: BTreeMap<String, f32>,
    held: BTreeSet<String>,
    just_changed: BTreeSet<String>,
    /// Buttons that went down at any point this frame, each with the set of
    /// buttons that were already held **at that instant**.
    ///
    /// The snapshot is the point: a press is latched for the whole frame, so
    /// reading its modifiers later would compare a press from early in the
    /// frame against a keyboard state from the end of it.
    pressed_this_frame: BTreeMap<String, BTreeSet<String>>,
}

impl InputState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a button transition.
    ///
    /// Repeats are ignored: a held key that the OS auto-repeats must not look
    /// like a fresh press, or every `Pressed` action fires continuously.
    pub fn set_button(&mut self, button: &str, down: bool) {
        let was_down = self.held.contains(button);
        if was_down == down {
            return;
        }
        self.just_changed.insert(button.to_owned());
        if down {
            // Snapshot BEFORE inserting, so a button is never its own
            // modifier, and remembered for the whole frame rather than only
            // until the release: `Pressed` used to mean "down AND changed", so
            // a key tapped and released between two redraws was never seen at
            // all — the faster you typed, the more input the editor dropped.
            let at_press = self.held.clone();
            self.held.insert(button.to_owned());
            self.pressed_this_frame.insert(button.to_owned(), at_press);
        } else {
            self.held.remove(button);
        }
    }

    /// Record an analog source's deflection, -1..1 for a stick, 0..1 for a
    /// trigger.
    ///
    /// **Not cleared by [`end_frame`](Self::end_frame)**, deliberately: a stick
    /// pushed and held emits one event and then nothing, so a level that reset
    /// each frame would read as a tap.
    pub fn set_axis(&mut self, axis: &str, value: f32) {
        self.analog.insert(axis.to_owned(), value);
    }

    /// The signed value an action's analog bindings currently supply, after
    /// scale and dead zone. Zero when the action has no analog binding, which
    /// is what keeps every keyboard-only context behaving exactly as before.
    #[must_use]
    pub fn analog_of(&self, map: &ActionMap, context: &str, action: &str) -> f32 {
        let Some(ctx) = map.contexts.get(context) else {
            return 0.0;
        };
        let Some(bindings) = ctx.axes.get(action) else {
            return 0.0;
        };
        let mut best = 0.0_f32;
        for b in bindings {
            let raw = self.analog.get(&b.axis).copied().unwrap_or(0.0) * b.scale;
            if raw.abs() >= b.dead_zone && raw.abs() > best.abs() {
                best = raw;
            }
        }
        best
    }

    /// Clear the per-frame transition set. Call once per frame, after reading.
    pub fn end_frame(&mut self) {
        self.pressed_this_frame.clear();
        self.just_changed.clear();
    }

    /// Whether an action is firing in `context`.
    #[must_use]
    pub fn is_active(&self, map: &ActionMap, context: &str, action: &str) -> bool {
        map.is_active(
            context,
            action,
            &self.held,
            &self.just_changed,
            &self.pressed_this_frame,
        )
    }

    /// `+1` for `positive`, `-1` for `negative`, `0` for both or neither —
    /// the shape movement code actually wants.
    #[must_use]
    pub fn axis(&self, map: &ActionMap, context: &str, positive: &str, negative: &str) -> f32 {
        let p = f32::from(self.is_active(map, context, positive));
        let n = f32::from(self.is_active(map, context, negative));
        let buttons = p - n;
        // **Whichever is deflected further wins, rather than summing.** A hand
        // on the keyboard and a thumb on the stick should not give 2.0, and
        // whichever the player is actually using is the one further from rest.
        // A stick bound to the *positive* action supplies the whole signed
        // value, so binding it to both would double it -- that is why the
        // negative side is subtracted rather than taken as a second candidate.
        let analog = self.analog_of(map, context, positive) - self.analog_of(map, context, negative);
        if analog.abs() > buttons.abs() { analog } else { buttons }
    }
}

/// Gamepads, polled into an [`InputState`] — ADR 0086.
///
/// **Names, not codes, because the rest of this crate is names.** A pad button
/// arrives as `PadSouth` and a stick as `PadLeftStickX`, so a binding file
/// spells a controller exactly the way it spells a keyboard and the action
/// layer never learns that gamepads exist. `{:?}` on `gilrs`'s own enums is the
/// source of those names: they are stable, documented, and one fewer table to
/// keep in step by hand.
///
/// **Every pad is one player here.** `gilrs` reports which device an event came
/// from and this throws that away, because nothing in the engine yet has a
/// second player to give it to. Local co-op is where that becomes a lie, and it
/// is the first thing to change here when it does.
pub struct Gamepads {
    inner: gilrs::Gilrs,
}

impl Gamepads {
    /// Open the gamepad subsystem, or report why not.
    ///
    /// **Failure is normal and must not be fatal.** A headless box, a container
    /// with no `/dev/input`, or a CI runner all fail here, and none of them
    /// should stop a scene from rendering. The caller is expected to carry an
    /// `Option<Gamepads>` and simply have no pads.
    pub fn open() -> Result<Self, String> {
        gilrs::Gilrs::new()
            .map(|inner| Self { inner })
            .map_err(|e| e.to_string())
    }

    /// Drain pending events into `state`.
    ///
    /// Call once a frame, before reading actions. Buttons go through
    /// [`InputState::set_button`] so they get the same press/held/released
    /// treatment as a key, and axes through [`InputState::set_axis`].
    pub fn pump(&mut self, state: &mut InputState) {
        while let Some(event) = self.inner.next_event() {
            match event.event {
                gilrs::EventType::ButtonPressed(button, _) => {
                    state.set_button(&pad_button_name(button), true);
                }
                gilrs::EventType::ButtonReleased(button, _) => {
                    state.set_button(&pad_button_name(button), false);
                }
                gilrs::EventType::AxisChanged(axis, value, _) => {
                    state.set_axis(&pad_axis_name(axis), value);
                }
                // `ButtonChanged` carries a trigger's analog travel. Recorded as
                // an axis so a trigger can drive a throttle, while the digital
                // press above still fires an action.
                gilrs::EventType::ButtonChanged(button, value, _) => {
                    state.set_axis(&pad_button_name(button), value);
                }
                _ => {}
            }
        }
    }

    /// How many pads are connected right now.
    #[must_use]
    pub fn connected(&self) -> usize {
        self.inner.gamepads().count()
    }
}

/// `Button::South` → `"PadSouth"`.
#[must_use]
pub fn pad_button_name(button: gilrs::Button) -> String {
    format!("Pad{button:?}")
}

/// `Axis::LeftStickX` → `"PadLeftStickX"`.
#[must_use]
pub fn pad_axis_name(axis: gilrs::Axis) -> String {
    format!("Pad{axis:?}")
}

/// The bindings shipped with the engine, so a fresh checkout has a working
/// camera without anyone writing a config first.
pub const DEFAULT_BINDINGS: &str = include_str!("../../../assets/input/default.toml");

#[cfg(test)]
mod tests {
    use super::*;

    const PAD_MAP: &str = r#"
[contexts.play.actions]
forward = [{ button = "KeyW" }]
back = [{ button = "KeyS" }]

[contexts.play.axes]
forward = [{ axis = "PadLeftStickY", scale = -1.0 }]
"#;

    /// **The shipped scheme must actually reach a controller.** Every other
    /// test here builds its own map, so all of them would still pass with the
    /// gamepad bindings deleted from `default.toml` — and a player would plug in
    /// a pad and find nothing moved. This is the row that fails instead.
    #[test]
    fn the_shipped_bindings_drive_a_gamepad() {
        let map = ActionMap::from_toml(DEFAULT_BINDINGS).expect("shipped bindings");
        let mut state = InputState::new();
        // Left stick forward, in both contexts a player can be in.
        state.set_axis("PadLeftStickY", -1.0);
        for context in ["play", "fly"] {
            let v = state.axis(&map, context, "move_forward", "move_back");
            assert!(
                (v - 1.0).abs() < 1e-6,
                "{context}: a stick pushed forward should read +1, got {v}"
            );
        }
        // And a face button, which goes through the ordinary button path.
        state.set_button("PadSouth", true);
        assert!(
            state.is_active(&map, "play", "jump"),
            "PadSouth should jump in the shipped scheme"
        );
    }

    /// Layering is per action: rebinding one must not erase its neighbours.
    #[test]
    fn an_overlay_replaces_one_action_and_leaves_the_rest() {
        let mut map = ActionMap::from_toml(DEFAULT_BINDINGS).expect("shipped bindings");
        map.overlay(
            ActionMap::from_toml("[contexts.play.actions]\njump = [{ button = \"KeyJ\" }]\n")
                .expect("overlay parses"),
        );
        let mut state = InputState::new();
        state.set_button("KeyJ", true);
        assert!(state.is_active(&map, "play", "jump"), "the rebinding applies");
        state.set_button("KeyJ", false);
        state.set_button("KeyW", true);
        assert!(
            state.is_active(&map, "play", "move_forward"),
            "an action the overlay never mentioned is untouched"
        );
        state.set_axis("PadLeftStickX", 1.0);
        assert!(
            (state.axis(&map, "play", "move_right", "move_left") - 1.0).abs() < 1e-6,
            "axes survive an overlay that only names an action's buttons"
        );
    }

    /// **A stick pushed forward reports negative Y**, so every scheme that
    /// wants forward-is-positive says `scale = -1.0`. Getting this wrong is a
    /// controller that walks backwards, which is why it is pinned rather than
    /// left to whoever writes the next bindings file.
    #[test]
    fn a_stick_supplies_a_signed_value_through_scale() {
        let map = ActionMap::from_toml(PAD_MAP).expect("map parses");
        let mut state = InputState::new();
        state.set_axis("PadLeftStickY", -1.0);
        assert!((state.axis(&map, "play", "forward", "back") - 1.0).abs() < 1e-6);
        state.set_axis("PadLeftStickY", 1.0);
        assert!((state.axis(&map, "play", "forward", "back") + 1.0).abs() < 1e-6);
    }

    /// A stick at rest is never exactly zero. Without a dead zone the character
    /// walks across the room while nobody holds anything.
    #[test]
    fn a_resting_stick_reads_zero() {
        let map = ActionMap::from_toml(PAD_MAP).expect("map parses");
        let mut state = InputState::new();
        state.set_axis("PadLeftStickY", -0.08);
        assert_eq!(state.axis(&map, "play", "forward", "back"), 0.0);
    }

    /// Whichever input is deflected further wins. A hand on the keyboard and a
    /// thumb on the stick must not add up to 2.
    #[test]
    fn keyboard_and_stick_do_not_sum() {
        let map = ActionMap::from_toml(PAD_MAP).expect("map parses");
        let mut state = InputState::new();
        state.set_button("KeyW", true);
        assert!((state.axis(&map, "play", "forward", "back") - 1.0).abs() < 1e-6);
        // Stick half forward while W is held: the key is still further out.
        state.set_axis("PadLeftStickY", -0.5);
        assert!((state.axis(&map, "play", "forward", "back") - 1.0).abs() < 1e-6);
    }

    /// **The regression that matters most:** every context authored before
    /// gamepads existed has no `axes` table, and must behave exactly as it did.
    #[test]
    fn a_context_with_no_axes_is_unchanged() {
        let map = ActionMap::from_toml(
            "[contexts.play.actions]\nforward = [{ button = \"KeyW\" }]\nback = [{ button = \"KeyS\" }]\n",
        )
        .expect("map parses");
        let mut state = InputState::new();
        assert_eq!(state.axis(&map, "play", "forward", "back"), 0.0);
        state.set_button("KeyS", true);
        assert!((state.axis(&map, "play", "forward", "back") + 1.0).abs() < 1e-6);
        // An analog source nothing is bound to changes nothing.
        state.set_axis("PadLeftStickY", -1.0);
        assert!((state.axis(&map, "play", "forward", "back") + 1.0).abs() < 1e-6);
    }

    /// A key tapped and released between two redraws is still a press. It used
    /// to be `down && changed`, so the release erased it and the action never
    /// fired — the faster you typed, the more the editor dropped.
    #[test]
    fn a_key_pressed_and_released_in_one_frame_still_fires() {
        let map = super::ActionMap::from_toml(super::DEFAULT_BINDINGS).expect("shipped bindings");
        let mut input = super::InputState::new();

        input.set_button("Delete", true);
        input.set_button("Delete", false);

        assert!(
            input.is_active(&map, "edit", "delete"),
            "a full tap inside one frame must register"
        );
        input.end_frame();
        assert!(!input.is_active(&map, "edit", "delete"), "and only once");
    }

    /// A modified action must see the modifiers that were held **when the key
    /// went down**, not whatever is held when the frame is read. Latching the
    /// press without latching its context meant a key tapped early in a frame
    /// could be claimed by a modifier pressed later in the same frame — Ctrl+S
    /// saving because you happened to reach for Ctrl after tapping S.
    #[test]
    fn a_modifier_pressed_after_the_key_does_not_claim_it() {
        let map = super::ActionMap::from_toml(super::DEFAULT_BINDINGS).expect("shipped bindings");
        let mut input = super::InputState::new();

        // S tapped on its own, then Ctrl goes down — all before the redraw.
        input.set_button("KeyS", true);
        input.set_button("KeyS", false);
        input.set_button("ControlLeft", true);

        assert!(
            !input.is_active(&map, "edit", "save"),
            "S was pressed unmodified; Ctrl arriving later must not make it a save"
        );
    }

    /// And the chord that *is* held together must still fire, even when the
    /// whole thing happens inside one frame.
    #[test]
    fn a_chord_tapped_inside_one_frame_fires() {
        let map = super::ActionMap::from_toml(super::DEFAULT_BINDINGS).expect("shipped bindings");
        let mut input = super::InputState::new();

        input.set_button("ControlLeft", true);
        input.set_button("KeyS", true);
        input.set_button("KeyS", false);
        input.set_button("ControlLeft", false);

        assert!(
            input.is_active(&map, "edit", "save"),
            "Ctrl was held at the moment S went down, so this is a save"
        );
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn map() -> ActionMap {
        ActionMap::from_toml(
            r#"
[contexts.fly.actions]
move_forward = [{ button = "KeyW" }, { button = "ArrowUp" }]
move_back = [{ button = "KeyS" }]
sprint = [{ button = "ShiftLeft" }]
reframe = [{ button = "KeyF", trigger = "pressed" }]
drop_item = [{ button = "KeyG", trigger = "released" }]
screenshot = [{ button = "KeyP", modifiers = ["ControlLeft"], trigger = "pressed" }]

[contexts.menu.actions]
close = [{ button = "Escape", trigger = "pressed" }]
"#,
        )
        .expect("valid bindings")
    }

    #[test]
    fn an_action_fires_from_any_of_its_bindings() {
        let map = map();

        assert!(map.is_active("fly", "move_forward", &set(&["KeyW"]), &set(&[]),
            &std::collections::BTreeMap::new()));
        assert!(map.is_active("fly", "move_forward", &set(&["ArrowUp"]), &set(&[]),
            &std::collections::BTreeMap::new()));
        assert!(!map.is_active("fly", "move_forward", &set(&["KeyS"]), &set(&[]),
            &std::collections::BTreeMap::new()));
    }

    /// The point of contexts: the same action name is inert outside its own.
    #[test]
    fn an_action_is_inert_in_another_context() {
        let map = map();

        assert!(!map.is_active("menu", "move_forward", &set(&["KeyW"]), &set(&[]),
            &std::collections::BTreeMap::new()));
    }

    /// `Pressed` fires on the transition only. Auto-repeat must not retrigger.
    #[test]
    fn pressed_fires_once_and_held_keeps_firing() {
        let map = map();
        let mut input = InputState::new();

        input.set_button("KeyF", true);
        assert!(input.is_active(&map, "fly", "reframe"), "fires on press");

        input.end_frame();
        assert!(
            !input.is_active(&map, "fly", "reframe"),
            "must not fire again while held"
        );

        input.set_button("KeyW", true);
        assert!(input.is_active(&map, "fly", "move_forward"));
        input.end_frame();
        assert!(
            input.is_active(&map, "fly", "move_forward"),
            "held keeps firing"
        );
    }

    #[test]
    fn released_fires_on_the_way_up() {
        let map = map();
        let mut input = InputState::new();

        input.set_button("KeyG", true);
        input.end_frame();
        assert!(!input.is_active(&map, "fly", "drop_item"));

        input.set_button("KeyG", false);
        assert!(input.is_active(&map, "fly", "drop_item"));
    }

    /// A repeated down event is not a fresh press — otherwise OS key repeat
    /// makes every `Pressed` action fire continuously.
    #[test]
    fn auto_repeat_is_not_a_new_press() {
        let map = map();
        let mut input = InputState::new();

        input.set_button("KeyF", true);
        input.end_frame();
        input.set_button("KeyF", true); // the OS repeating

        assert!(!input.is_active(&map, "fly", "reframe"));
    }

    #[test]
    fn a_modifier_is_required_when_declared() {
        let map = map();
        let mut input = InputState::new();

        input.set_button("KeyP", true);
        assert!(
            !input.is_active(&map, "fly", "screenshot"),
            "Ctrl is required"
        );

        input.set_button("ControlLeft", true);
        assert!(input.is_active(&map, "fly", "screenshot"));
    }

    /// Modifiers are a requirement, not an exclusion: sprinting forward is
    /// still moving forward.
    #[test]
    fn an_unrelated_modifier_does_not_block_a_plain_binding() {
        let map = map();

        assert!(map.is_active(
            "fly",
            "move_forward",
            &set(&["KeyW", "ShiftLeft"]),
            &set(&[]),
            &std::collections::BTreeMap::new()
        ));
    }

    #[test]
    fn an_axis_reads_minus_one_zero_or_one() {
        let map = map();
        let mut input = InputState::new();

        assert_eq!(input.axis(&map, "fly", "move_forward", "move_back"), 0.0);
        input.set_button("KeyW", true);
        assert_eq!(input.axis(&map, "fly", "move_forward", "move_back"), 1.0);
        input.set_button("KeyS", true);
        assert_eq!(
            input.axis(&map, "fly", "move_forward", "move_back"),
            0.0,
            "both pressed cancels"
        );
        input.set_button("KeyW", false);
        assert_eq!(input.axis(&map, "fly", "move_forward", "move_back"), -1.0);
    }

    /// An action with no bindings can never fire; keeping it silently makes a
    /// typo look like a broken key.
    #[test]
    fn an_empty_binding_list_is_rejected() {
        let err = ActionMap::from_toml("[contexts.fly.actions]\njump = []\n")
            .expect_err("empty binding list");

        assert!(format!("{err}").contains("can never fire"));
    }

    /// E is the interact key while playing, and it is `pressed` — a held key
    /// reaching a door script sixty times a second is the bug this exists to
    /// prevent. It is `move_up` in the `fly` context and must stay there:
    /// contexts are what let one key mean two things.
    #[test]
    fn e_interacts_while_playing_and_still_flies_up() {
        let map = ActionMap::from_toml(DEFAULT_BINDINGS).expect("shipped bindings");
        let mut input = InputState::new();

        input.set_button("KeyE", true);
        assert!(input.is_active(&map, "play", "interact"), "E must interact");
        assert!(input.is_active(&map, "fly", "move_up"), "E must still fly up");

        input.end_frame();
        assert!(
            !input.is_active(&map, "play", "interact"),
            "held E must not interact twice"
        );
    }

    /// **The M6 exit criterion.** The shipped bindings load and drive a camera.
    #[test]
    fn the_shipped_bindings_load_and_cover_the_camera() {
        let map = ActionMap::from_toml(DEFAULT_BINDINGS).expect("shipped bindings must be valid");

        let actions: Vec<&str> = map.actions_in("fly").collect();
        for required in [
            "move_forward",
            "move_back",
            "move_left",
            "move_right",
            "move_up",
            "move_down",
            "sprint",
            "look",
            "reframe",
            "quit",
        ] {
            assert!(actions.contains(&required), "missing action: {required}");
        }
    }
}
