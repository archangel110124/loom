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

/// A named set of actions that are live together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Context {
    /// Action name → the ways it can fire.
    #[serde(default)]
    pub actions: BTreeMap<String, Vec<Binding>>,
}

/// Every context, as loaded from TOML.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
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
    ) -> bool {
        let Some(bindings) = self.contexts.get(context).and_then(|c| c.actions.get(action)) else {
            return false;
        };

        bindings.iter().any(|binding| {
            // Modifiers are a requirement, never an exclusion: binding W with
            // no modifiers still fires while Shift is held, because sprinting
            // forward is still moving forward. A binding that needed exclusion
            // would say so, and none has yet.
            if !binding.modifiers.iter().all(|m| held.contains(m)) {
                return false;
            }
            let down = held.contains(&binding.button);
            let changed = just_changed.contains(&binding.button);
            match binding.trigger {
                Trigger::Held => down,
                Trigger::Pressed => down && changed,
                Trigger::Released => !down && changed,
            }
        })
    }
}

/// Tracks which buttons are down, and which changed this frame.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    held: BTreeSet<String>,
    just_changed: BTreeSet<String>,
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
            self.held.insert(button.to_owned());
        } else {
            self.held.remove(button);
        }
    }

    /// Clear the per-frame transition set. Call once per frame, after reading.
    pub fn end_frame(&mut self) {
        self.just_changed.clear();
    }

    /// Whether an action is firing in `context`.
    #[must_use]
    pub fn is_active(&self, map: &ActionMap, context: &str, action: &str) -> bool {
        map.is_active(context, action, &self.held, &self.just_changed)
    }

    /// `+1` for `positive`, `-1` for `negative`, `0` for both or neither —
    /// the shape movement code actually wants.
    #[must_use]
    pub fn axis(&self, map: &ActionMap, context: &str, positive: &str, negative: &str) -> f32 {
        let p = f32::from(self.is_active(map, context, positive));
        let n = f32::from(self.is_active(map, context, negative));
        p - n
    }
}

/// The bindings shipped with the engine, so a fresh checkout has a working
/// camera without anyone writing a config first.
pub const DEFAULT_BINDINGS: &str = include_str!("../../../assets/input/default.toml");

#[cfg(test)]
mod tests {
    use super::*;

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

        assert!(map.is_active("fly", "move_forward", &set(&["KeyW"]), &set(&[])));
        assert!(map.is_active("fly", "move_forward", &set(&["ArrowUp"]), &set(&[])));
        assert!(!map.is_active("fly", "move_forward", &set(&["KeyS"]), &set(&[])));
    }

    /// The point of contexts: the same action name is inert outside its own.
    #[test]
    fn an_action_is_inert_in_another_context() {
        let map = map();

        assert!(!map.is_active("menu", "move_forward", &set(&["KeyW"]), &set(&[])));
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
            &set(&[])
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
