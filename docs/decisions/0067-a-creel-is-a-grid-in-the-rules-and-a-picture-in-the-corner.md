# ADR 0067 — A creel is a grid in the rules and a picture in the corner

- **Date:** 2026-08-21
- **Status:** **accepted** and built. `deeper_demo.loom` ships it; it is the
  only inventory in this tree and it replaces the four counters that were.
- **Sits under:** ADR 0045, clause 2 — the creel is readable by `--assert` and
  by rhai, so it is a deterministic CPU function of (scene, tick), stepped
  inside the fixed step. It is: it is rhai inside `Runner::tick`. It is
  deliberately **outside** `state_hash`, which is clause 1 obeyed rather than
  evaded; see *The determinism ruling* below.
- **Applies to:** `loom_script::Motion::bag`, `loom_cli::play::PlayerInput::bag`,
  `loom_cli::hud::{Creel, Hand, creel}`, `assets/input/default.toml`,
  `assets/scripts/deeper_rules.rhai`, `assets/scripts/deeper_player.rhai`,
  `assets/scripts/fish_hang.rhai`, and §6 of `scripts/green.sh`.

## The request

> "I would like to have, like, a Resident Evil style inventory. And since this
> is gonna be a multiplayer game, I would like for it to have no pausing while
> in the inventory."
>
> "Inventory system, go very deep on it. Make sure it is polished and perfect."

Two requirements, and the second is the hard one. Resident Evil's inventory is
*spatial* — a grid, items that occupy more than one cell, arranging and
combining, and the tension of deciding what to leave behind. And the world keeps
running while it is open, because you cannot pause a world other people are
standing in.

## Decision

**Nine cells, three by three, held as a list of placements in `GameRules` state,
driven from a seventh input channel, drawn as a painted 138 px square in the
corner of the overlay, and `Play::paused` is never touched.**

### 1. No `Inventory` component

The creel is `state.creel`, a rhai list of `#{ kind, x, y, rot }`. A
`Vec<Item>` inside a `loom_scene` component gets no schema validation — nested
component fields are not range-checked — so an `Inventory` component would cost
six files for the appearance of safety and buy range-checking on two integers
that a rhai constant already fixes. Add it the day a second scene wants a
different creel; then it is `{ width, height }` and four lines.

**Occupancy is one bitmask per row, folded from the list every tick and never
stored.** Two representations of one fact is the bug this system is most likely
to have, and `state.creel_drift` — cells the grid says are taken, minus the sum
of `w*h` over the list — is the number that would catch it. The bounds test runs
**before** the AND, or a 2-wide item at x = 2 in a 3-wide grid wraps into the
next row and reports a clean fit.

### 2. A seventh digital channel, not a second meaning for E

`bag`, on Tab, through the same six places `interact` went through in `2248af1`.
An inventory is a *mode*: while it is open `interact` means "lift or place", and
the key that opened it has to still be able to close it. One key cannot be both,
and a long-press on E is a gesture nothing in this project can test headlessly.

Tab is free while a game is running, checked rather than assumed: `handle_editing`
returns the moment `Play` exists, and egui's unconditional Tab consumption is
already un-consumed by hand in the viewer.

### 3. The cursor is on W/A/S/D, and his legs stop

The only directional input in this engine is `move_x` / `move_z`. Opening the
creel takes them; `speed` is zeroed on `bag_open` before it is zeroed on
`at_helm`. Measured on one tape, W held from tick 0 in both runs: **z is -12.308
with the grid shut and -3.073 with it opened at tick 60**, and *exactly* -3.073
at tick 400 and again at tick 900.

**The shut figure is the character controller's and it moves.** It read -9.310
when this was written; `40bd02d` gave the view mass and it became -12.308,
which the gate did not notice because the row asserts `< -9.0`. That slack is
deliberate — the claim is "he walked a long way" against "he did not move" —
but it means the number in this paragraph is a snapshot and the *stop* is the
part that is pinned.

**The pointer stays captured, so mouse-look keeps working.** That is the
difference between this and the pause menu, and it is the whole feel of the
decision: you can sweep the horizon, watch the rod tip and see another player go
over the side with the grid up. Releasing the pointer — the pause menu's
gesture — would zero all seven channels *and* stop `DeviceEvent::MouseMotion`,
leaving you unable to walk **and** unable to look, which is strictly worse than
the pause it is avoiding.

**Nine cells partly so that a keyboard cursor is a d-pad rather than a wade.**
A mouse cursor is refused for now: `--hold` cannot press a mouse, so it would be
a system with zero automated coverage of its central interaction.

**Refused at the wheel.** W/A/S/D are the throttle and the rudder on the helm
mat; `at_helm` — the movement script's own "the wheel is answering you" — removes
the design's worst input collision with one condition, diegetically. It is
re-checked every tick and not only on the press, because the mat is on a rigid
body and can arrive under a man whose legs are frozen.

### 4. The world does not stop, and the proof needed no new code

`Play::paused` is untouched. Two rows in §6 say what that costs the player:

- Cast, open the creel, wait. The take still arrives (`events.bite`) and is
  still missed (`events.spooked`), because LMB turns the item in your hand while
  the grid is up and cannot also strike a fish.
- Hook one, open the creel, stop reeling. `SLACK_LIMIT` is still counting:
  `events.escaped`, and the phase back to 0.

**Rummaging mid-fight costs you the fish.** That consequence was already built,
which is why nothing here has to force the creel shut on a bite — a force-close
would need a rules→movement channel this engine does not have, and its own
proposed assertion would pass while the player was soft-locked.

### 5. What is in it

Four 1×1 supplies — bait, line, lantern, thermos — picked up on the walk to the
boat, so **four of nine cells are gone before you fish**, and the free five are
not a rectangle. A fish is rolled on the bite to one of four sizes: sprat 1×1,
runner 2×1, hake 2×2, conger 2×3.

A conger is six cells and does not fit beside a kit. The grid needs no rule
saying "a big fish is hard to carry" — it simply is one.

**Three curios are aboard when she is refused and only two of them are answers.**
The creel reads `a.b/c../...` — thermos (0,0), line (2,0), lantern (0,1) — and
all three solves are measured in §6 rows 5b4 and 5b9:

| ditch | taps to reach it | free cells | she goes in |
| --- | --- | --- | --- |
| **thermos** (0,0) | 0 — the cursor is already on it | six, in an L | **no**. No 2×3 and no 3×2 in them |
| **lantern** (0,1) | 1 tap of S | rows 1–2, a clean 3×2 | yes, **turned**: `a.b/ccc/ccc` |
| **line** (2,0) | 2 taps of D | columns 1–2, a clean 2×3 | yes, **unturned**: `acc/bcc/.cc` |

*Which* curio goes over the side is a spatial question with a wrong answer, and
the wrong answer is the one nearest to hand. The cheapest right answer is the
one that only works because the packer may turn her. **This paragraph had the
thermos and the lantern the wrong way round until a row was written for each**,
which is the argument for writing the row and not the paragraph.

**The rod is not an item.** `rod_hold.rhai` puts it in his hands, in the world,
where he can see it. **There is no combine and no knife**: a whole extra verb to
solve a space problem that ditching already solves. **There is no auto-sort**,
now or later, on any argument — it is a solver for the system's only puzzle, and
pressing it becomes always-optimal the moment it exists. If packing hurts, the
creel is one cell too small; fix that.

### 6. A fish that will not fit stays on the LINE

This reversed once, during the build, and the reversal is the ADR.

The first version put her in the player's **hand** — off the line, off any
timer — on the argument that a fish left on the rod is a fish under pressure.
**Measured: she is not.** `phase` only resets when `online == 0`, so a landed
fish hangs off the tip indefinitely; traced to tick 3900, two thousand after the
landing, still there.

And the hand version was a **dead end**. You have one hand. With the conger in
it you cannot lift a curio to repack and you cannot ditch one either, so the
only move left inside the creel is to throw away the fish you just fought for.
On the line she waits while you empty a cell with both the hand and the ditch
free.

**So the ditch reads the cursor as well as the hand**, for the same reason. With
an empty hand, SHIFT over a cell puts *that* item over the side, and that is the
move the whole puzzle turns on.

### 7. The keys, and why each one

| key | closed | open |
| --- | --- | --- |
| **Tab** | open the creel (refused at the helm) | close it, and what is in your hand goes back by first fit |
| **W A S D** | walk / helm | step the cursor. Edge at \|axis\| > 0.5, then repeat after 20 ticks every 8 |
| **E** | take / stow, unchanged | lift what the cursor is over; place it if it fits, refuse with a code if not |
| **LMB** | cast | turn the held item — never one already placed |
| **Shift**, held 60 ticks | sprint | over the side. Irreversible, so it is a held key and never one tap |
| **Space** | jump, or the helm release | ignored. **Not the ditch** — on the mat it releases the boat |
| **Escape** | pause menu | pause menu, over the creel, unchanged |

Shutting with a full hand puts the item back rather than refusing to shut: a
held item has no picture and no verb outside the grid, and a TAB that stops
working reads as a bug rather than as a rule. It always fits — it came out of
this grid one press ago and its own cells are still free.

### 7b. Why the last press did nothing, and when it stops being true

Five codes and no others. The list in the script named a `4` nothing set, was
missing `5`, and described `2` as its own opposite.

| | | |
| --- | --- | --- |
| **1** | it will not go there | E, hand full, the footprint overlaps |
| **2** | nothing in that cell | E or SHIFT, hand empty, cell empty |
| **3** | hands on the wheel | TAB on the helm mat |
| **4** | nothing in your hand | LMB with an empty hand |
| **5** | it is one cell | LMB on something that cannot turn |

**4 exists because 2 was lying.** LMB with an empty hand answered *nothing in
that cell*, which is false whenever the cursor is on something — and the player
it is answering is exactly the one who reached for the turn key before the lift
key, i.e. the one looking at an item.

**Every code is a fact about the cell under the cursor, so all of them expire
when the cursor moves.** `IT WILL NOT GO THERE   LMB turns it — or move the
cursor` told the player to do a thing and doing it changed nothing: he walked
the fish to a free cell, the overlay painted her footprint green, and the line
went on saying she did not fit. Two readouts of one fact, disagreeing, and the
wrong one made of words.

**The clear is tested after the clamp**, on `cx != cx0 || cy != cy0` and not on
the arrival of a `cursor` event. Pressing D at the right-hand wall emits an
event and moves nothing, and a message that blinks off when nothing moved is the
same bug the other way round. Both injected, both caught by 5b10.

The latch itself stays, and stays for `--assert`'s sake: `--assert` runs once,
at the end of a run, so a per-tick flag written on tick 550 and cleared on 551
reads 0 at tick 600 and the row passes having checked nothing.

### 8. Painted, never an `egui::Area`

`Hud` is `anchor`, `offset`, `text`, `size`, `color`. A grid of multi-cell items
is not expressible in it and never will be, so `hud::creel` is the second thing
`loom_cli` draws over a game — and it is drawn exactly the way the HUD is.

**An `egui::Area` claims `wants_pointer_input` across its whole rect even with
painted-only content.** Injected, and it fails
`nothing_in_the_creel_claims_the_pointer` while leaving
`nothing_in_the_creel_claims_the_keyboard` green: an empty `Area` eats the
pointer and not the keys. The two tests guard two different mistakes.

**No scrim and no panel.** 138 px in a corner is 0.9% of a 1920×1080 frame. The
demo's frame is open water, the water is what this engine is being built to
show, and in a world that does not stop, a grid over the middle hides the thing
you most need to see.

**One outline per item, not one per cell.** The grid is projected as one letter
per *placement* — never per kind — so the border between two cells is drawn iff
their characters differ: one four-neighbour test, no extra data, and two sprats
side by side draw as two objects. A joined 3×2 is a ten-segment perimeter; six
separate items are twenty-four.

ASCII in `Monospace`, and no emoji: a glyph the overlay's font does not have
renders as a plain box and passes every text assertion.

## The determinism ruling

**ADR 0045 clause 2 is satisfied.** The creel is readable by `--assert` and by
rhai, so it must be a deterministic CPU function of (scene, tick), stepped
inside the fixed step. It is. There is no RNG in scripts: the size roll uses the
file's own hand-written LCG on a **private stream** seeded `90210 + tick`, taking
bits 16–22 rather than the low bits, so that `state.landed_tick == 1345` and
`state.fight_ticks == 1177` do not move. No `HashMap` iteration, no `thread_rng`,
no wall clock. It produces no force on a rapier body: a fish in a bag has no
mass in this engine.

**It does not enter `state_hash`, and that is clause 1 obeyed rather than
evaded.** `World::state_hash` is physics-only and does not grow. Putting a
pickup in it would re-pin every sibling hash in the repository every time a fish
changed size, converting a gameplay tune into a workspace-wide re-pin. Clause 1
is about how a value is *computed*, not about what the hash eats.

**The cost, stated plainly:** `cargo test`'s determinism check and
`cargo xtask repeat` are blind to the creel. Two runs could hold different fish
and both report green. **`loom sim --assert` is the only detector**, and it only
sees what the script remembered to project out — which is the fight's bargain,
for the fight's reason, and is why every row landed in `scripts/green.sh` in the
same pass rather than later.

## What the gates see, and what they cannot

`cargo xtask image` and `cargo xtask repeat` see **nothing**, structurally.
`Ui::new` occurs exactly once in this tree and it is behind a
`winit::window::Window`; the offscreen renderer never constructs one. **The 54
references cannot move because of this overlay. If one moves, the change touched
something else and the change is wrong — do not `--bless` it.**

But `deeper_demo.loom` **is** in `SCENES`, so it is loaded, validated and
rendered on every `cargo xtask image` run. A scene edit that fails to load, or a
`fish_hang.rhai` scale that produced degenerate geometry, would break the gate
even though no reference PNG exists for it. That is the one way this work can.

**And the drawing has one detector that is not a unit test.** Eleven unit tests
prove the layout and the shapes against a headless `egui::Context`, and every
one of them would still pass if the rules script renamed `creel_cx` tomorrow:
`Creel::read` would return `None`, the grid would vanish, and nothing anywhere
would say so. §6's row 5b8 checks all ten names the overlay reads against one
real run of the real game. It is the cheapest possible test and it is the only
thing standing between the two halves.

## Multiplayer, and the three shape decisions that are free today

- **Every mutation is a discrete, tick-stamped event carrying its cell** —
  `place{x,y,rot}`, `lift{x,y}`, `ditched{item}` — never a continuous drag and
  never a pointer position. That is also exactly what keeps it inside the fixed
  step, and it is the second reason the cursor is on the keyboard.
- **The cursor and the open flag are per-player view state.** They live in
  `state` today because that is the only storage `--hold` can drive and
  `--assert` can read; when replication arrives they are not sent — a remote
  client does not need to know where your cursor is.
- **Cross-player grid browsing and drag-between-bags are refused.** They are
  unsolvable while nothing pauses, and the shipped answer everywhere it has been
  tried is a *world* action: you leave it on the deck.

`play.rs` still clones one `PlayerInput` into every character, so none of this
is testable yet.

**The brief asked for the per-player key on day one and it was not done, and
that is a deviation rather than an oversight.** `state.creel` is flat. Keying it
`state.creels.p0` is a map lookup today and a search-and-replace through fifteen
hundred lines the day a second player exists, so the cost of deferring is real
and known. It was still deferred: there is no second player, no replication, and
no test that could tell the two shapes apart, so the key would be structure
bought entirely on faith — and the *hard* part of a second creel is not where it
is stored but that `play.rs` has one `PlayerInput`, which the rename does not
touch. **Do it in the same commit as the second player, not before.**

## Rejected

- **A movement-script `state` channel**, so the rules could force the creel
  shut. An engine change at the base of the dependency graph, reopening a
  documented boundary, to buy one behaviour the world already punishes.
- **The rod as a grid occupant.** It is in his hands, visibly, in the world.
- **A knife, a fillet, any combine at all.** A whole extra verb for a space
  problem ditching already solves.
- **The boat hold as a second grid.** `stowed` stays a counter and one E at the
  hatch still empties everything fishy. Two grids and a cross-grid cursor before
  anyone has held the first one.
- **An auto-sort button.** See §5.
- **Item stacking, quantities, weight, durability, tooltips, drag-and-drop,
  right-click menus, and hit-testing of any kind.** Nothing in the creel is
  interactive to the pointer; that is what makes the pointer test possible.
- **An open/close animation.** The shut plate is drawn dimmer instead.

## What no gate in this repository can answer

Four things, and they need a human at `loom run`:

1. Whether the plate is dark enough over bright water.
2. Whether 38 px cells read at 1080p and at 1440×900.
3. Whether the cursor's 20/8-tick repeat feels right. Both constants sit alone
   at the top of `deeper_player.rhai` so tuning them is an edit and not a
   rebuild. `--assert` can confirm the cursor reached x = 1; it cannot tell
   whether it got there when you asked.
4. Whether the frozen legs read as tense or as a pause with extra steps. If they
   read as a pause, the redesign is real rather than a tune: the creel becomes
   openable only at rest or at the hatch, and the field inventory becomes
   something smaller. Budget for it before calling this done.
5. Whether the held item's footprint preview, which is **clamped to the grid**,
   reads as "it hangs off the edge" or as "it shrank". It is red in exactly that
   case, so the answer is on screen; whether it is the answer a player reads is
   not something a shape count can say.

## What the refinement pass found

**Rotation had zero coverage.** `cells_of`'s `rot` branch could be deleted, or
`first_fit` stopped from ever trying the turned footprint, and the whole
gameplay block still exited 0 — because the one conger any row landed goes in
*unturned* and no row had ever placed anything else. §5's table is the fix, and
§6 rows 5b9 now cover the packer choosing a rotation, LMB changing the
footprint in the hand, `creel_fits` flipping at a fixed cell because of it, and
the refusal that follows. All three injections caught.

Two comments were also measured and found stale — the walk figure in §3, and
this ADR's own thermos-versus-lantern sentence, which was backwards.
