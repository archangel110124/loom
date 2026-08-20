# DEEPER — the fishing loop, as built

The mechanical design of the core loop, and the record of what was cut and what
would bring it back. `DEEPER-GDD.md` §5 is the brief; this is the ruling on it
and the shape of the thing that now runs.

**Three facts to hold before reading anything else.**

1. **`state_hash` is physics-only.** The whole fight lives in a rules script's
   `state`, so every determinism check in this project is blind to it: clippy
   sees no Rust, the determinism hashes see only rigid bodies, and the image
   gate photographs a scene whose rules could have changed underneath it.
   **`loom sim --assert` is the sole detector**, which is why the gate block at
   the end of `scripts/green.sh` exists and why it is not optional.
2. **The GDD's §5.4 is kept as a sequence, not a chord.** Every element it asks
   for survives — line stress, fish stamina, directional pull, rhythm windows,
   drag — but as phases with *one dominant demand live at a time*. Four
   concurrent demands is four things watched and none read.
3. **Water in any scene meant to be asserted against is `deterministic`.**
   ADR 0053's `cinematic` tier is read back off the GPU and `loom sim` refuses
   the whole run rather than answer a `water@` assertion approximately. The
   beauty shot, when there is one, gets its own file.

---

## 1. The ruling on §5.4

The GDD justifies its four-demand fight with "research says pure reaction-tests
fatigue but rhythm stays fresh". **That is a designer's bet wearing a
citation's coat** — the cited taxonomy contains no such claim. The bet may well
be right, and it is worth making; it is not evidence, and the design should not
be built as though the question were settled.

So the fight is built as the *test* of that bet: a stress-tracking spine with
rhythm windows layered on top, where the windows are the part that can be
measured. The measurement is `WINDOW_CALM` (below), which makes "did the player
find the rhythm layer" a number rather than an opinion.

**What the sequencing buys.** At any instant the player is doing one thing:
holding tension, or reacting to a run, or taking a window. Counter-steer and
live drag are not cut from the design — they are queued behind a phase table
(§6), because adding a second live axis to a fight that already has one is how
a legible mechanic becomes a mush.

## 2. The loop

Six phases, two verbs.

| phase | the player | ends on |
| --- | --- | --- |
| 0 idle | press fire | cast |
| 1 waiting | **nothing — this is the serenity beat**, 120–300 ticks (2–5 s) | bite |
| 3 bite | press fire inside 18 ticks (300 ms) | hooked, or spooked back to idle |
| 4 fighting | hold forward to reel; fire takes a window | snap / escape / landed |
| 5 landed | — | `status = "won"` |
| 6 lost | — | `status = "lost"` |

Taught in one sentence: **hold W to reel — it drains the fish and loads the
line; let go to bleed; when the fish runs, let go.** Learned in one fight:
**windows only open on a line you kept calm.**

`move_x`, `sprint` and `jump` are deliberately unspent. Counter-steer, drag and
cut-the-line land there later without an engine change and without moving what
the player already learned.

## 3. Every constant

In `assets/scripts/fishing_fight.rhai`. Rates are per second, integrated at the
fixed timestep. Meters are 0..100 floats because `Hud` renders `{name}` as an
integer and a 0..1 fraction would display as `0`.

**Stress** — `+22` reeling, `+18` during a run, `+15` more for reeling *into*
one (55/s together: 1.27 s from a comfortable 30 to a snapped line, which is
the greed line), `−30` slack. Snaps at 100.
**The 22:30 ratio is the difficulty dial, not the absolutes** — it fixes the
reachable duty cycle at 58%, and scaling both changes nothing.

**Stamina** — `−11` reeling calm, `−15` reeling through a run, `+2` recovering
on slack. Landed at 0. `DRAIN_CALM` is the single fight-length knob and is
near-linear.

**Runs** — 90 ticks long, 100–220 apart, about 30% of a fight. **`TELL_TICKS`
is 24: 400 ms of warning, and it is the fairness constant.** Shrinking it is
the depth-zone difficulty axis. Removing it turns a run into a hidden-state
reaction test, which is the one thing the research does condemn.

**Windows** — open when `stamina < 45 && !running && stress < 50`, for 15 ticks
(250 ms) every 90. A hit is −8 stamina and −10 stress; a miss is +6 stress and
**must be edge-detected**, because a held button is otherwise sixty mispresses
a second.

**`WINDOW_CALM = 50` is the load-bearing clause and it was added by
measurement.** It is why a pilot who wins by brute grip earns zero windows
while one who keeps the line calm earns three, and that gap is the skill
gradient stated as an integer a machine can read.

**Slack escape** — stress under 5 for 300 ticks (5 s) loses the fish to
`escaped`. **An unattended rod must lose the fish and must never snap.**

**Randomness is a 32-bit LCG**, `(s * 1103515245 + 12345) % 2147483648`, with
its first thirty-two rolls folded into `state.lcg32` and pinned in the gate.
Never float hashing: `frac(sin(x))` is libm-dependent and would replay a fight
differently on another machine, which is exactly what `loom_field::noise`'s
frozen-ABI rule exists to prevent. The multiplier stays 32-bit because rhai's
arithmetic is checked and a 64-bit constant throws on the first multiply.

## 4. How a fight is asserted

**The problem.** `loom sim` never calls `set_input` — headless input is zeros
forever. And `--assert` runs once, at the end of the run, against final state.

**The answer is to swap the pilot and never the model.** Five closed-loop
reference policies live inside `fishing_fight.rhai`, where fight state is
visible, and a scene selects one by containing a node named
`Root/Pilot/<name>`. An input tape was rejected: it cannot react to an
LCG-scheduled run, and it rots the moment a constant moves. A policy cannot go
stale, because it reads the same state a human reads off the meter.

The node-name trick also defers a `params` field on `Script` indefinitely — the
five benches differ from each other by one word, and no scene needs a second
script file.

Everything time-varying accumulates into `state` as it happens: `peak`, `hot`,
`hits`, `missed`, `runs`, and the `s120`/`s300`/`s600` tick probes. Those probes
print in every run's `game` block and are the fight's readable diff.

**Measured, and reproduced across three independent re-runs:**

| pilot | outcome | fight ticks | peak stress | windows hit | note |
| --- | --- | --- | --- | --- | --- |
| skilled | **lands it** | 1080 | 70.3 | 3 | 0 mispresses, 4 runs survived |
| lazy | snaps | 199 | 100.4 | 0 | holding the reel down loses |
| masher | snaps | 80 | 101.3 | 0 | 10 mispresses — worse than doing nothing |
| idle | escapes | 313 | 11.5 | 0 | never snaps |
| sloppy | **lands it** | 1177 | 88.9 | **0** | hot for 921 ticks |

**The lazy/skilled pair is the design's first pillar as a regression test.**
Same fish, same schedule, opposite outcomes. If both ever win, the fight has
stopped being a decision and become a slot pull, and the gate says so with
nobody watching.

**The sloppy row is the gradient.** Two hundred milliseconds late on everything
still wins — a fight nobody survives badly is a wall, not a skill check — but it
wins slower, spends most of its length with the line hot, and earns *nothing*.

### Two kinds of gate row, and writing only one of them was a mistake

The wide bands say the **design** still holds. The two exact pins —
`landed_tick == 1345` and sloppy's `fight_ticks == 1177` — say the **numbers**
did not move.

Only the wide bands were written first, and a deliberate 18% change to
`DRAIN_CALM`, the single fight-length knob, **passed every one of them**: the
fight ran 822 ticks instead of 1080 and no band noticed. The pins are the
wind-hash precedent applied here. Retuning the fight means re-pinning those two
numbers in the same commit, deliberately — a readable line in a diff instead of
a silent drift.

**Cross-scene orderings cannot be asserted** (`--assert` sees one run), so
"skilled finishes before sloppy" and "masher before idle" belong in a future
`cargo xtask play`, not in a scene.

## 5. What is in the scene, and what is in the script

**Scene** — deterministic `WaterBody`; the boat as a **prefab instance**
(`assets/prefabs/jib_vi.loom`, never an edit to `jib_vi_painted.loom`); the
angler as `CharacterController` + `Script`, which is the only thing in the
engine that sees a key press; the lure as a sphere with `RigidBody`, `Buoyancy`
and `Submersion`, authored above the water so it falls and splashes on its own;
an **authored `Camera`**; one `Hud` line.

**Script** — the whole model in one file, necessarily: the sandbox sets
`set_max_modules(0)`, so there is no `import` and two copies could not be kept
in sync. `fishing_angler.rhai` is six lines and forwards two fields as an
event, which the rules script reads on the **same tick** (the motion pass runs
before the rules pass).

**Engine Rust: none.** That is the whole point and it held.

**The meter is an ASCII bar assembled in the rules script** and rendered through
one `{message}` line. `Hud` has no bar component and does not need one — a bar
cannot change the tuning, so it must not be allowed to delay it.

### A trap this cost, worth not paying twice

`[[asset]].id` is optional and defaults to the empty string, and prefab asset
merging keys identity on exactly that id. **So every id-less asset declaration
in a prefab is the same asset as every id-less declaration in the scene
instancing it**, and the first adopted wins for all of them. The boat's nine
meshes were silently rewritten to the fishing scene's `sphere` and drawn as
nine coincident unit spheres at the waterline. The scene validated clean,
reported the correct object count, and put the hull at the correct position.
**Give every `[[asset]]` a real `id`.** The engine ought to refuse an empty one
rather than alias on it; that is an open validation gap.

### Measured boat geometry, for whoever builds the rod station

Deck top is **y = 1.40**. The deck mesh's outboard edge is at **|z| = 2.839**
amidships (x ≈ −3..−1), tapering to **2.750** by x ≈ −6. A rail or rod station
at |z| = 2.90 — as first specified — hangs 6 cm outboard of the deck it is
supposed to stand on. **Stations amidships, at |z| ≤ 2.80.** The angler
currently stands at boat-local `[1.20, 2.30, 2.60]`: feet on the deck, inboard
of the edge, parented to the hull so the roll comes free.

## 6. Cut, and what brings each back

| cut | returns when |
| --- | --- |
| Cargo Tetris (§5.7) | the fight is fun **and** a second fish exists. Own slice; a `state` grid and `{message}` first, no UI. |
| Counter-steer and live drag | with the phase table. **Drag first, as a once-per-run commitment**, never as a second live axis. Line spool returns with it — loose-forever must cost something. |
| Lure jigging (§5.2) | if the bite wait exceeds ~8 s. Needs mouse-delta reconstruction; the bindings are digital. |
| Aim and cast power (§5.2) | with the second cast, via `detonate` for the splash — verified that `apply_blast` filters on a body's **centre**, so a 0.6 m blast at the lure cannot reach a hull whose centre is metres away. Falsify that in a short probe before building on it. **No impulse binding.** |
| Seven of the eight archetypes (§5.5) | **Sulker second**, because it inverts the verb — reeling snaps, waiting lands — and so proves the phase table composes rather than adding another parameter tuple. Lurer third, as the tonal-shift tool. |
| Co-op, Anchor, tag-team (§5.6) | not on the current stance, and for **two** reasons rather than one: there is no networking, *and* no per-player input path — every character is handed the same input. Both are holes. |
| Rod progression, persistence (§6) | a save format exists. The `params`-on-`Script` gap is flagged; nothing is built for it. |

## 7. Engine asks, in order

Nothing here is needed for the slice that runs today.

1. **Event-triggered `AudioSource`** (~40 lines, finishing a component that is
   already documented). The 400 ms tell is the fairness constant and a line of
   text is its weakest possible carrier.
2. **A node `Script` that can read `state`**, read-only and one tick stale.
   Needs an ADR and an adversarial test. It is what lets a fish node move with
   the fight it is losing.
3. **A `Tether` line component** (~120 lines of Slang, reusing the grass Bézier
   and the rain ribbon's 2.5 px width floor; the pointer goes in the environment
   buffer because the push block is at 124 of its 128 guaranteed bytes). This is
   the one that makes the rod point at something, and it joins `GOLDEN` — with a
   `shimmer` number at the authored camera — in the commit it ships in.

**Why the fishing scene is not in `GOLDEN` yet.** The HUD is egui-only and
invisible headless, and a boat on water is already covered by
`jib_vi_painted.loom`. An entry now would report a pass without ever having
looked at the thing under test, which is the defect grass paid for twice.
