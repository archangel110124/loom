# ADR 0086 — A stick is not a button

- **Date:** 2026-09-06
- **Status:** **accepted** — fourth of the nine subsystems scoped as "what is
  missing for a full-blown game engine", at the general-engine bar.
- **Decision touched:** adds one dependency, `gilrs`, pinned exactly per the
  rules. Nothing in the locked table moves.
- **Human decisions this records:** `gilrs` rather than raw evdev, and bindings
  as a scene component with a user override file rather than user config alone.

## 1. What was already there, and what I got wrong about it

The gap was reported as "keyboard and mouse only, no gamepad, **no rebinding**".
The second half was wrong, and the error is worth recording because it is the
same shape as several others in this project: the grep was for `rebind|keybind`
and the feature is called `ActionMap`.

`loom_input` already had rebindable named actions, contexts, press/held/released
triggers, modifiers, TOML loading — and it was *wired up*, not inert:
`run.rs` loads `assets/input/default.toml` with a built-in fallback. Its header
even anticipated this work: *"This crate knows nothing about `winit`: it takes
opaque button names as strings."*

So a gamepad button did not need a new concept. It needed a name.

## 2. Three things were actually missing

**Analog.** `set_button` is a boolean and `axis()` synthesised an axis from two
of them. A stick reports -1..1 and a trigger 0..1, and squashing either into a
boolean at the driver throws away the half of the signal that makes a controller
worth having — walking slowly is the feature.

`AxisBinding` is therefore a separate type from `Binding`, and `Context` gains
an `axes` table beside `actions`. It defaults, so every context authored before
gamepads existed parses and behaves exactly as it did — pinned by
`a_context_with_no_axes_is_unchanged`.

**Layering.** Bindings came from one fixed engine-wide path, so a game could not
ship its own scheme without replacing everyone else's, and a player could not
rebind anything without editing the file the game shipped. There are three
layers now — engine, then the scene's `Bindings` component, then
`$XDG_CONFIG_HOME/loom/bindings.toml` — and `overlay` replaces **per action**,
not per context or per file: rebinding `jump` costs you nothing else, and axes
layer independently of buttons on the same action.

**The device.** `Gamepads` wraps `gilrs` and pumps events into `InputState`.

## 3. The decisions inside it

**Names, not codes.** A pad button arrives as `PadSouth` and a stick as
`PadLeftStickX`, from `{:?}` on gilrs's own enums. A bindings file spells a
controller exactly the way it spells a keyboard, the action layer never learns
gamepads exist, and there is one fewer hand-kept table to fall out of step.

**Keyboard and stick do not sum.** Whichever is deflected further wins. A hand
on the keyboard and a thumb on the stick must not give 2.0, and whichever the
player is actually using is the one further from rest.

**`scale = -1.0` is not a preference.** A stick pushed forward reports
*negative* Y. Every scheme that wants forward-is-positive says so in the
bindings rather than in game code, and it is pinned by a test, because getting
it wrong is a controller that walks backwards.

**A dead zone, defaulting to 0.15.** Measured: the pads this was written against
wander to about 0.08 at rest. Without it a character walks across the room while
nobody is holding anything.

**No pads is not an error.** `Gamepads::open` fails on a headless box, in a
container with no `/dev/input`, and on CI. The caller carries an `Option` and
reports once at startup. A scene must still open.

**Pumped at the top of the frame**, before `step_camera` and the play input path
read the map. An event drained after either waits a whole frame — on a stick
that is stale deflection, on a button it is a press the player made and did not
get.

## 4. What is tested, and the one test that matters most

Six new tests. Five check the mechanism; the sixth checks the *product*:
`the_shipped_bindings_drive_a_gamepad` drives `DEFAULT_BINDINGS` rather than a
map built in the test, because every other test would still pass with the
gamepad bindings deleted from `default.toml` — and a player would plug in a pad
and find nothing moved. That is the inert-field failure this project keeps
meeting, and it is the row that fails instead.

## 5. Not built

- **One pad, one player.** `gilrs` reports which device an event came from and
  this throws it away. Local co-op is where that becomes a lie, and it is the
  first thing to change here when there is a second player to give it to.
- **No rumble**, though `gilrs` offers it.
- **No in-game rebinding UI.** Rebinding is editing a text file, which is the
  same answer this engine gives for everything else authored — but a shipped
  game usually wants a menu, and that belongs with the game UI layer.
- **No stick look in `fly`.** Deliberate: the viewer's look is hold-to-orbit on
  the right mouse button, and a stick that orbits whenever it is touched would
  fight the gizmo drag sharing the screen. `play` has it.
