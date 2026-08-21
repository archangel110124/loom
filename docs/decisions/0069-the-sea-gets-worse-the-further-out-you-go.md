# ADR 0069 — The sea gets worse the further out you go

- **Date:** 2026-08-21
- **Status:** **accepted**
- **Decision touched:** none of CLAUDE.md's locked decisions moves. Builds on
  ADR 0068's operator; adds no pass, image, descriptor, barrier or trait.
- **Applies to:** `Environment.stages`, `Environment.dread`, `loom_cli::mood_of`
  and `deeper_rules.rhai`.

## Context

DEEPER is a horror fishing game. The brief, in the human's words: *"the further
you go out, the more eerie it will become. So at the beginning it should look,
you know, happy, bright — not too bright, but normal."*

That is a grade that **moves**, driven by gameplay, blending smoothly. It is a
storytelling device rather than a filter.

## Decision

**A list of named stages on the component that already owns the sky, blended on
the CPU by one scalar.**

A `MoodStage` is a name, a position `at` on a `0.0 -> 1.0` axis, an
`EnvironmentPatch` and a `Grade`. `mood_of` finds the bracketing pair, blends
their **parameters**, and returns the effective environment component *and* the
grade together — a caller that took the lighting from one `dread` and the grade
from another would render a frame nobody authored, with no way to notice.

### A stage, not a grade

Measured both ways. `Environment` owns darkness, fog and cloud, and no grade can
fake any of them. The grade owns saturation and the temperature of a warm prop,
and no `Environment` setting can touch either — **every "eerie" lighting
configuration anyone measured came out *more* colourful than the happy
baseline**, because taking a warm sun away stops washing albedos toward a common
white. Split into two systems, a designer authors one mood in two files and
discovers by hand that the fog is fighting the crush.

### A list, not two endpoints

Arithmetic rather than taste. The mid-water cast has to **peak** near the middle
and drain to neutral by the end. Lerping `sky_zenith` from `[0.16, 0.30, 0.52]`
to `[0.02, 0.03, 0.03]` gives `[0.09, 0.165, 0.275]` at the midpoint — still
blue-dominant, still a pretty sea — where the authored middle is green-dominant.
**A two-endpoint lerp cannot produce a peaked hue at all.**

### Parameters, never two images

A crossfade of two differently-hued frames cancels chroma: its midpoint measures
*less* saturated than either endpoint — 7.91 against 8.29 and 11.97. That
undershoot is a muddy midtone, and blending numbers cannot produce it. It is
also why there is no 3D LUT: 32,768 lines of floats defeats the project's first
property.

### A patch, not a duplicate

A key a stage does not name falls back to the scene's own component **on both
sides of the blend**, so it stays put rather than ramping from a default nobody
authored. That is why `sun_direction` appears once in `deeper_demo.loom` and not
five times — and the title shot is composed against that vector, so a sign flip
on `z` turns the rig into a small beige object on flat grey water.

### `dread` is authored, not a flag

On the component, so a still can be rendered anywhere on the ramp, the golden
row is a scene file, and the editor is WYSIWYG on the mood. A flag no scene
carries would be a second source of truth.

## The one trap, and it is the whole determinism argument

**`dread` eases in the rules script, on the fixed tick, and nowhere else.**

`run.rs` advances `wind_seconds` by a frame delta. A ramp riding that would pass
`cargo xtask image` (one frame), pass `cargo xtask repeat` (headless, fixed
step), and be **wrong only in the window the human judges everything in**. This
repo already carries that scar: `gleamsprat`'s `weather.z` was `--sim N / 60` in
the gate and a free-running clock in the viewer.

Both readers — the offscreen path and the viewer — *read* `state.dread` from the
running script's `GameState` and neither eases it. `loom render --sim N` drives
the same `play::Runner` as `loom sim`, so `state.dread` after N ticks is a pure
function of (scene, N).

## The typo gate, which is the S4 bug in a new place

`loom_reflect::validate` walks a component's **top-level** keys only. Every
number this design adds lives inside nested tables, so `sun_strenth = 0.4`
inside a stage is a key nothing checks: dropped at load, the stage renders the
near look at every `dread`, and the scene reports `{"ok": true}`.

`EnvironmentPatch` and `Grade` carry `deny_unknown_fields`, and `check_moods`
deserialises the component at load so the refusal actually fires. There is a
test for it, and **removing `deny_unknown_fields` fails that test** — checked,
not assumed. Two further refusals: a one-stage ladder is a constant that looks
like a ramp, and two stages at one `at` is a zero-span divide.

## The stages, and what only a human can judge

Five rungs on `deeper_demo`: `inshore` (0.0), `underway` (0.25), `outofsight`
(0.5), `grounds` (0.8), `theedge` (1.0).

- **`inshore` is a positive grade with no brightness change.** Warm tilt, +12%
  saturation, `contrast` neutral. ADR 0068's operator already moves this frame's
  median up a few codes and the brief said "not too bright", so the near end must
  not add to it. One control answers both requests, in opposite directions.
- **`outofsight` is not dark** — it is the third-brightest rung, and it is the
  beat nobody expects. A bilious green sky over a sea the colour of an
  uncleaned aquarium, with the deck you are standing on still lit.
- **The green is `sky_zenith`, and blue is the trap.** At matched luminance teal
  reads as dusk and is rather beautiful — it is *Abzû* — grey reads as a dull
  day, rust reads as a sunset. Desaturated yellow-green is the only one of the
  four that is *unpleasant*, and it lands only paired with the overcast ceiling
  and the saturation drop.
- **`exposure` is the master dimmer and the one knob that reaches the cloud
  deck.** `scene.slang` carries fixed cloud tones, so no amount of turning the
  sun down darkens an overcast sky. `exposure` is applied in the tonemap,
  downstream of the deck — which is why `theedge` sits at 0.48.

## The ramp's pacing, measured

She spawns 11 m out and makes **2.97 m/s** at full ahead — 29.7 m per 600 ticks,
measured. The first distances tried (near 60, far 260) left the opening 50 m of
travel moving nothing at all: seventeen seconds of holding W with no answer, and
a far end past a minute and a half. **25 and 150** put the first change about
five seconds out — while the rig is still over her shoulder, which is the brief
— and the far end around fifty seconds.

Rising fast and falling slow: 0.020/tick reaches 90% in ~1.9 s out, 0.004/tick
takes ~9.6 s back. That asymmetry is Dredge's panic meter, and it is why turning
for home reads as relief rather than a light switch.

## Consequences

- **Zero golden references move.** Every scene in the library authors no stages,
  the grade is the exact identity, and the shader detects that with a uniform
  branch rather than an epsilon. Verified: all 54 rows re-rendered, 0 moved.
- **The sim hash does not move.** `tower` is `b478ea4ac2622d32` and
  `deeper_demo --ticks 600 --hold move_z=1` is `db3c34df6ea61acf`, both
  unchanged. `Play::state_hash` is physics-only and `fn sim` never constructs a
  `Renderer`.
- **Two `green.sh` §6 rows gate the chain without pixels**, at 2.6 s total: the
  berth is pinned at the floor, and thirty seconds of full ahead reaches 0.33
  against a 0.1 threshold. No pixel row can see this — `deeper_demo` is in
  `SCENES` and not `GOLDEN`, and one still is one point on a ramp anyway.

## Open, and only a human can close it

**Every stage was judged as a still.** The `at` values and the two easing
constants are invention. `cargo xtask flythrough` cannot answer it, because the
mood does not move during a flythrough. If driving out reads as a light switch
or as nothing happening, the easing constants are one line of rhai and the stage
values are numbers in a TOML file — `deeper_demo` is not in `GOLDEN`, so tuning
either re-blesses nothing. **That is precisely why the stage values are authored
text and landed last.**

The known missing beat is the near/far split — *the sea goes wrong while your
own deck is still lit*. It arrives partly for free from the geometry of fog. Do
not add a second scalar for it speculatively.
