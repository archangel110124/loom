# ADR 0066 — A title is a camera, a curtain, and a line the scene wrote

- **Date:** 2026-08-21
- **Status:** **accepted** and built. The convention below is what
  `deeper_demo.loom` uses today and what any second game in this tree would
  have to follow.
- **Sits under:** ADR 0045, which the whole front end stays outside — a camera
  and an overlay rectangle are rendering, not simulation, and nothing here is a
  function of tick. ADR 0017, whose "a frame is no longer a pure function of its
  tick" objection does **not** apply: the curtain runs on the wall clock but is
  drawn only by `loom run`, which no gate photographs.
- **Applies to:** `loom_cli::hud` (`title_scrim`, `title_menu`, `curtain`,
  `fade`), `loom_cli::run` (`TITLE_CAM`, `Front`, `front_end_wanted`,
  `title_answers`), `loom_scene::components::Hud::only_on_title`, and
  `assets/games/deeper_demo.loom`.

## The request

> "I'd like to make, like, a cinematic camera controller. So when we're
> starting, maybe it's looking down at the ocean, and it says, do you want to
> start the game or something like that? Or it says, welcome to DEEPER, or just
> say DEEPER. Start new, options, quit… And then once it does the transition to
> the game, it'll go to where the player would be. And let's do some transitions
> as well, like fade in, fade out — I think fade in, fade out is probably the
> only things you're really gonna need for games like this."

## Decision

**There is no `Shot` component, no `Cinematic`, no camera animation and no
transition system.** A front end is four things a scene already had a way to
say, plus two functions:

1. **A camera node named `Rig/TitleCamera`, authored `active = false`.**
   That is the whole declaration. `active = false` keeps it out of
   `World::active_camera` and out of `player_character`'s walk, so it renders no
   pixel and fails no assertion; `World::camera_named` reads it by path.
2. **The game's name and its controls as `Hud` elements with
   `only_on_title`.** Not a `GameRules.title` field and not a string in
   `loom_cli`.
3. **One curtain function**, `hud::curtain(elapsed)`, run on the wall clock.
4. **Two buttons and an Escape hint**, which are the engine's, not the scene's.

### Why the last two sentences of that list are the decision

**Chrome belongs to the engine; content belongs to the scene.** The split is not
aesthetic. `escape_means(false, false, false)` is `Close` — the engine decides
that and the scene cannot change it, so the engine says it. What a key *does* in
a particular game, the engine cannot know: `run::PLAY_KEYS` says "left click to
fire" because `fire` is a channel name, and in this demo that channel casts a
rod. A scene that wants a player to read something true writes it as a `Hud`
line, laid out by the same `draw` as the score, with `anchor`, `offset`, `size`
and `color` authored. `loom_cli` holds no game's name and no game's verbs.

The seam between them is one documented number: **a title screen owes the bottom
32 px to the engine's Escape hint**, and `deeper_demo` puts its two legend lines
at 40 and 68 px up.

### `only_on_title` is not the inverse of `only_in_play`

There are three states, not two. Playing; on the title screen; and neither —
which is `loom run` with no game at all, and is the editor. A title over the
editor is the bug `front_end_wanted` exists to stop, and an element flagged
`only_in_play = false` would have been drawn there.

## Fades are the only transition, and the exponent is not a taste knob

The request named fade in and fade out and said they are probably all this kind
of game needs. That is taken as the scope.

`hud::fade` paints black over the whole window in `Order::Tooltip`, above both
menus. **A painter and not an `Area`**: a full-viewport interactive region once
ate every click in this codebase.

`curtain` is `1 - (1 - t)^2.2` and the exponent is the display gamma, not a
preference. The overlay is drawn premultiplied (`src ONE / dst
ONE_MINUS_SRC_ALPHA`) into a `B8G8R8A8_SRGB` swapchain, and Vulkan converts an
sRGB attachment to linear before blending and back after — so alpha scales
*radiance*. Displayed brightness therefore goes as `(1 - a)^(1/2.2)`, and with
that exponent it falls **exactly linearly in `t`**. A linear ramp in alpha
instead leaves the screen at 73% of its brightness at the halfway point of its
own fade and then plunges: nothing, nothing, slam. The instinct at that point is
to lengthen the duration, and the duration was never the problem.

**One curve, two entrances.** The window opens with the clock already at
`FADE_OUT + HOLD` — the top of the fade-*up* — so the same function that takes
the picture away brings it in, and there is no second timeline to keep in step.
That is also what puts the half second a scene takes to load behind a curtain
rather than in front of one.

**The expensive work goes in the hold, which is a floor and not a ceiling.**
`start_play()` runs on the first frame of the black. If building the world costs
longer than 0.25 s, the black lasts longer. That is the design, not a race.

## What is refused, and why

**No camera animation.** The sea already moves — the water's clock is the wall
clock, and 72% of pixels change over a tenth of a second at the title camera —
so the shot is alive for free. A drift is three lines of `sin()` in a `Front`
method if a human looks at it and says it reads dead. Building it first would be
building the thing that cannot be judged without the window nobody can open.

**No Options.** There is no settings system in this tree — no persisted config,
no volume, no keybind store. An Options item is a promise made on the first
screen a player ever sees and broken on the second. The one thing a player would
have opened it for is the controls, and those are on the title screen instead.
The pause menu made the same call one commit earlier, and a title offering
Options over a pause menu that does not would have the two disagree about what
this is.

**No second scene file, no `loom render --camera`, no audio fade, no `ease`
enum, no render-graph fade pass.** Each is a generalisation of something with
one caller.

## Consequences, including the unwelcome one

**Adding a front end moves `World::state_hash`.** That hash eats every entity's
index, generation, name and world matrix — it is not physics-only. Four nodes
took `deeper_demo` from `1bae49bdb57408c0` to `9c6af005fafd052d`, and reordering
them would move it again. It moves **no pixel** (0 differing of 256,000 at zero
tolerance) and **no gameplay assertion** (all four `green.sh` rows pass with the
nodes and without), and no gate pins this scene's hash, so nothing is broken.
But "a camera is rendering-only and cannot affect determinism" was written here
once and is false. A scene whose hash *is* pinned must re-pin it in the same
commit that gives it a title screen.

**`--play` now has a state the window never had**: viewer up, no game running,
indefinitely, with the file watcher and the fly camera both live. It is the same
state as plain `loom run`, so it is supported — but nothing had exercised it
under `--play` before.

**The buttons are drawn under the opening curtain and do not answer.**
`Order::Tooltip` blocks no input, so gating the *drawing* would have made the
menu pop in when the fade finished. `title_answers` gates the answer instead.

## What no gate in this project can check

`loom run` opens a real window; this box has no Xvfb; nothing here can
photograph a `Ui`. `cargo xtask image` drives the offscreen `Renderer`, which
never constructs one, so **no reference PNG can move and none can catch a
regression either**. `run::the_demo_opens_on_a_camera_the_game_never_uses` is
the only thing that fails if the shot, its lens, its bearing or any of the three
lines of text quietly stops existing — the failure mode a front end is most
exposed to, and the one the golden gate has been fooled by twice already.

The parts that were judged by compositing the real alphas over the real render,
in linear light, with egui's own font: the framing, the word's readability
(7.6:1 against the water), the legend's fit, and the evenness of both fades.
The parts that only a window can settle: the *pace* of 0.5 / 0.25 / 0.8, whether
`start_play` fits the hold on a 214-node scene, whether the first gameplay
frame's pipeline warm-up lands inside the black, the pointer hand-off under the
curtain, and whether a static camera over a moving sea reads as alive.

**Doing that compositing on the bytes instead of in linear was the one real
trap.** It makes every fade look twice as dark at its halfway point, and it
argues for retuning a curve that is correct. It is the same defect as measuring
anti-aliasing on a path the player never sees.
