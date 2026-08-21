#!/usr/bin/env bash
# The definition of green. All four, every time. `cargo check` passing is not green.
set -euo pipefail
cd "$(dirname "$0")/.."

./scripts/check-deps.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Vulkan validation. Needs a GPU + the layers; skips honestly without either.
if cargo metadata --no-deps --format-version 1 | grep -q '"name":"xtask"'; then
  cargo xtask validate
  # The pixel diff. Clippy catches what the compiler misses, the validation
  # layers catch what clippy misses, the determinism hashes catch a simulation
  # that drifted — and none of them notices a shader that now renders
  # everything slightly wrong. Skips honestly without a GPU, same as validate.
  cargo xtask image
  # **Byte-identity across processes** (ADR 0045 clause 3). `image` compares at
  # a calibrated tolerance, which is the right question for "did the picture
  # change" and cannot see "is the picture the same twice". Every GPU-stateful
  # path in the engine — the drop buffer, the particle pool — is licensed by
  # that second property, and until this existed it was checked by hand once.
  cargo xtask repeat
else
  echo "skip: cargo xtask validate — xtask crate does not exist yet (M2)"
fi

# ---------------------------------------------------------------------------
# 6. Gameplay. **Nothing above this line has ever run a game.**
#
# `state_hash` is physics-only, so a scripted game loop can be rewritten from
# end to end and every check above still passes: clippy sees no Rust, the
# determinism hashes see only bodies, and the image gate photographs a scene
# whose rules changed underneath it. `loom sim --assert` is the only detector
# there is, and until this block existed it was in no gate at all — five
# gameplay systems (rules, enemies, damage, the event log, the fight) shipped
# covered by nothing but a human running the binary by hand.
#
# **The block takes about three and a half minutes**, and the comment here said
# half a second until someone timed it. 148 `loom sim` runs; a `deeper_demo`
# tick is roughly 1.3 ms and the demo tapes are two thousand ticks each, so the
# game rows are nearly all of it. That is a decision about the whole of section
# 6 and not about any one row — trim it by shortening tapes or by sharing runs
# (5b8 went from ten runs to one), never by dropping a claim.
#
# It is at the end because it needs a release binary, and it uses the one
# `cargo test` already built.
LOOM="${LOOM:-./target/release/loom}"
if [ ! -x "$LOOM" ]; then
  cargo build --release -j 3 -p loom_cli
fi

# **The fight, five pilots, one model.** `loom sim` never calls `set_input`, so
# headless input is zeros forever; the fight is tested by swapping the pilot
# and never the model. Each scene differs from the next by one word — the name
# of its `Pilot` node — and `fishing_fight.rhai` reads that out of `positions`.
#
# **The lazy/skilled pair is the design pillar as a regression test.** Both
# fight the same fish on the same schedule; one lands it and one snaps the
# line. If they ever both win, the fight has stopped being a decision and
# become a slot pull, and this says so unattended.
#
# **Two kinds of row, and the difference is the point.** The wide bands say
# the *design* still holds — a skilled pilot lands it, without snapping,
# taking windows. The two exact pins (`landed_tick`, `fight_ticks`) say the
# *numbers* did not move. Only the wide bands were written first, and a
# deliberate 18% change to `DRAIN_CALM` — the single fight-length knob — sailed
# straight through all of them: the fight ran 822 ticks instead of 1080 and
# every band still held. The pins are the wind-hash precedent applied here, so
# **retuning the fight means re-pinning these two numbers in the same commit,
# deliberately**, which is a readable line in a diff rather than a silent drift.
"$LOOM" sim assets/test/fight_skilled.loom --ticks 1800 \
  --assert "status == won" \
  --assert "events.snap == 0" --assert "events.escaped == 0" \
  --assert "state.peak < 100" \
  --assert "state.fight_ticks > 600" --assert "state.fight_ticks < 1300" \
  --assert "state.hits >= 2" --assert "state.runs >= 2" \
  --assert "state.missed == 0" \
  --assert "state.lcg32 == 16672" \
  --assert "state.landed_tick == 1345" >/dev/null

# Holds the reel down and reads nothing. Must lose, by snapping.
"$LOOM" sim assets/test/fight_lazy.loom --ticks 1800 \
  --assert "status == lost" --assert "events.snap >= 1" >/dev/null

# Mashing is worse than doing nothing — and the mispress that makes it so has
# to stay edge-detected, or a held button is sixty presses a second.
"$LOOM" sim assets/test/fight_masher.loom --ticks 1800 \
  --assert "status == lost" --assert "events.snap >= 1" \
  --assert "state.fight_ticks < 150" >/dev/null

# An unattended rod loses the fish and **never** snaps.
"$LOOM" sim assets/test/fight_idle.loom --ticks 1800 \
  --assert "status == lost" --assert "events.escaped >= 1" \
  --assert "events.snap == 0" >/dev/null

# 200 ms late on everything: still wins, but slower, hot for most of it, and
# earns zero rhythm windows. `hits == 0` here against `hits >= 2` on the
# skilled bench is the skill gradient stated as a number.
"$LOOM" sim assets/test/fight_sloppy.loom --ticks 2400 \
  --assert "status == won" --assert "state.hits == 0" \
  --assert "state.hot > 600" \
  --assert "state.fight_ticks == 1177" >/dev/null

# **A whole game, retro-covered.** `proving_ground.loom` has been in the repo
# since M12 with no gate on it whatsoever. Headless it is a player who never
# moves, so the enemies find him, damage him and kill him — which asserts
# navigation, enemy scripts, the damage path, the event log and `GameRules`
# win/lose in one 300-tick run. A weak claim, checked, beats a strong one
# nobody checks.
"$LOOM" sim assets/games/proving_ground.loom --ticks 600 \
  --assert "status == lost" --assert "events.damage >= 1" >/dev/null

# **The demo hub, and what a stranger can do in it.** `deeper_demo.loom` is
# where the fishing game is being built. `rig_walk`, `rig_drive` and
# `rig_overboard` are that same file with one or two fields changed — never a
# copy — because a test that is a copy of a level is worthless by the second
# week.
#
# **Two ways of driving a character appear below and the difference matters.**
# `rig_walk` swaps the movement model for a pilot script, which is right when
# the thing under test is *downstream* of a keypress. Everything else uses
# `loom sim --hold`, which writes `Runner::input` — the field `loom run` writes
# when a human holds W — and is the only shape that works when the thing under
# test **is the mapping from the press**. The helm reads `move_x` and `move_z`
# directly, so a pilot fabricating the thrust would test the pilot.

# The hub is crossable.
#
# **The four-slot claim used to live here and has moved**, to the shipped scene
# with real input, three rows down. It was proved by this pilot — which walks a
# lane it knows and picks things up along it — while the one gesture the demo
# actually teaches arrived at the boat carrying one of four. The supplies are
# now strung along that gesture instead, so `carried == 4` is a claim about a
# player rather than about a route the test wrote for itself. What is left here
# is the pickup path existing at all.
"$LOOM" sim assets/test/rig_walk.loom --ticks 220 \
  --assert "Rig/Player.x > 10.0" --assert "Rig/Player.y > 2.2" >/dev/null
"$LOOM" sim assets/test/rig_walk.loom --ticks 500 \
  --assert "state.carried >= 1" --assert "events.pickup >= 1" \
  --assert "Rig/Player.y > 2.2" >/dev/null

# **Thirty seconds of hopping into railings, and it is never lost.** This used
# to assert `y > 2.2` — "the railing held" — and passed by timing coincidence:
# the pilot turns away from a wall before a hop can coincide with being pressed
# against it, so it could not produce the failure it was written to detect.
# Pressed continuously, the capsule is over any rail in about thirty-five ticks.
# The rig's guarantee is no longer "you cannot get out", because the berth had
# to open for the boat to be boardable and no rail can beat the jump anyway. It
# is **"you always get back"**, and `y > -1.0` is a claim this pilot can fail.
"$LOOM" sim assets/test/rig_walk.loom --ticks 1800 \
  --assert "Rig/Player.y > -1.0" >/dev/null

# **The demo in one key.** From the spawn, holding W: past a supply, through the
# gap in the berth railing, over the bulwark, onto the cockpit deck, onto the
# helm mat, and away. `z < -11.0` is inboard of her port side deck; the y band
# is "standing on the deck" against 2.55 on the bulwark cap and -0.70 in the sea.
#
# **`state.at_helm` is the claim `Rig/Player.z < -11.0` was standing in for.**
# A position band is satisfied by a player standing anywhere in a strip of
# cockpit, and by a boat that drifted under one.
#
# **It changed meaning in round 4 and got better.** It used to be the rules
# script's own rectangle around the drawn mat — a *position*, which a player
# who has pressed SPACE and let go of the wheel still satisfies. It is now the
# fact `deeper_player.rhai` computes to decide whether W is the throttle,
# crossing to the rules as an event on change: **the wheel is answering you**.
# The fishing block below pairs it with the same hold plus SPACE, where the
# answer is no. Run 1 pairs it with tick 60, where it is also still no.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 60 --hold move_z=1 \
  --assert "state.at_helm == 0" >/dev/null
#
# **And the caption does not strobe on the way, which it did.** `aboard` was a
# raw per-tick ground test, so every airborne frame of the scramble over the
# bait box read as *not on the boat*: six caption flips in forty ticks on a
# dead-straight walk, the biggest text on the screen telling the player to walk
# forward to a boat he was standing on. It also silently dropped `fire` presses,
# because the same flag gates the cast.
#
# `events.station` is the whole count of station changes in the run — aboard,
# at the wheel, stalled, swimming, blocked. A clean walk-forward is **two**
# (aboard, then the helm) and this was **10**. It is 3, because the run starts
# by emitting the state it is already in.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 --hold move_z=1 \
  --assert "events.station <= 4" --assert "state.at_helm == 1" >/dev/null
#
# **420 and not the 240 it was, and the two seconds are the bait box.**
# `col_engine_box` sits amidships in the cockpit — boat-local x -8.40..-7.20 —
# and the boarding lane now runs at it rather than threading the 1.0 m gap
# between it and the deckhouse, because that gap is the narrowest thing on the
# whole route and aiming the demo down it is what made round 4's lane three
# degrees wide. Traced at `move_x = 0`: aboard at tick 180, shouldering the
# box's after face from 180 to 300, on the mat by 360. A player with a mouse
# steps round it without noticing; a dead-straight headless walk grinds.
# **That trade bought the lane its other twenty-one degrees** and it is the
# right way round.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 420 --hold move_z=1 \
  --assert "Rig/Player.z < -11.0" --assert "Rig/Player.y > 1.2" \
  --assert "Rig/Player.y < 1.9" --assert "state.at_helm == 1" >/dev/null

# **The kit, filled by the one gesture the demo teaches.** Nothing else is held
# and nothing is aimed: W, from the spawn, past four supplies laid along the way
# to the berth.
#
# **`state.full == 1` used to be here and its meaning changed, deliberately.**
# Four supplies in four slots was full; four supplies in a nine-cell creel is
# not, and "full" now means "nothing more will fit anywhere", which for a grid
# is the only honest form of the question. What replaces it is the row below —
# a *picture* of where the four landed, which is a much stronger claim than any
# count and is the one thing a stub cannot hardcode its way past.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 240 --hold move_z=1 \
  --assert "state.carried == 4" --assert "events.pickup >= 4" \
  --assert "state.bait == 1" --assert "state.thermos == 1" \
  --assert "state.creel_used == 4" --assert "state.creel_free == 5" >/dev/null

# **AND WHERE THE FOUR LANDED, WHICH IS THE CLAIM A NUMBER CANNOT MAKE.**
#
# `creel_cells` is one letter per *instance* per cell, rows joined by `/`, `.`
# for empty. `abc/d../...` is the first-fit packer's answer for four 1x1 items
# in a 3x3 grid and there is exactly one right string. A stub that hardcodes
# `creel_free` still has to hardcode this, and by then it has implemented the
# packer.
#
# **`--assert` has no string axis** — see `GameState::text` — so this is a grep,
# which is how every caption in this file is already pinned.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 240 --hold move_z=1 \
  | grep -q '"creel_cells": "abc/d\.\./\.\.\."'

# **THE PACKER'S TWO SELF-CHECKS, AND THEY ARE THE HALF OF THIS THAT IS PROVED
# RATHER THAN SAMPLED.**
#
# `creel_selftest` compares `fits_at`'s bit twiddling against a cell-by-cell
# scan of the same grid, one occupancy pattern per tick — all 512 reachable 3x3
# grids by tick 512, six footprints and nine origins each. `creel_drift` is
# `cells the grid says are taken` minus `sum of w*h over the placement list`:
# two numbers computed by different code from different data, equal on every
# path at every tick or the creel has double-booked or lost a cell.
#
# Both were fault-injected. Dropping the `x + w > gw` bounds test takes
# `creel_selftest` to 1398; making the overlap test pass instead of fail takes
# it to 10848 and `creel_drift` to -3, with `creel_cells` reading `d../.../...`
# — all four supplies stacked in one cell.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_z=1 \
  --assert "state.creel_selftest == 0" --assert "state.creel_drift == 0" >/dev/null

# ---------------------------------------------------------------------------
# **THE SEA GETS WORSE THE FURTHER OUT SHE GOES.**
# ---------------------------------------------------------------------------
#
# ADR 0069. `Environment.stages` names five looks along a 0->1 axis and
# `deeper_rules.rhai` eases `state.dread` along it from the boat's distance off
# the rig. **No pixel row can see this chain** — `deeper_demo` is in `SCENES`
# and not in `GOLDEN`, and even if it were, one still is one point on a ramp.
# These two rows gate script -> distance -> easing for the price of the CPU.
#
# Rising fast and falling slow is the design (Dredge's panic meter), so the
# near row is the one that catches an easing rate that has run away: at the
# berth `dread` must be pinned at the floor no matter how many ticks pass.
#
# **And the berth is dry**, which is the first sentence of the design: it is a
# normal morning at the rig and the weather arrives as you steam out. The calm
# rung says `rain_intensity = 0.0`, so this is free on the run above.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 60 \
  --assert "state.dread < 0.05" --assert "rain@3,3,-200.rate == 0.0" >/dev/null

# And the far row, which is the whole chain. **1800 ticks, and the number was
# measured rather than chosen:** she makes 2.97 m/s at full ahead, so thirty
# seconds puts her 74 m out and `dread` at 0.33 — a 3x margin over this
# threshold, at 2.4 s of wall clock. Saturation needs 3000 ticks and 4.1 s,
# which buys a rounder number and nothing else.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1800 --hold move_z=1 \
  --assert "state.dread > 0.1" >/dev/null

# **AND THE WEATHER RIDES THAT SAME SCALAR, WHICH NOTHING ELSE HERE CHECKS.**
# The rows above prove `dread` moves; this proves something is wired to it on
# the *simulation* side. Deleting the five `wind_speed` lines from the scene
# leaves every other row in this file passing — measured — because they read a
# mood, and a mood is a look.
#
# Measured on the shipped scene, holding W from the berth, `wind@3,3,-200`:
#
#     tick        60     1800     3000
#     dread    0.000    0.326    0.942
#     wind     2.475    4.258   11.372
#     rain     0.000    0.000   25.928
#     without  2.475    2.469    2.229   <- the control, ladder deleted
#
# The wind readings are below the authored `Wind.speed` because that is a
# free-stream value about 10% above U10 and the probe is at 3 m. **The
# `without` row goes *down*, which is the tell**: unramped, the only thing
# moving that number is gust phase.
#
# **The rain is on the same run and it is the second half of the ladder.**
# `mood_weather_of` returns a pair and every caller in the engine used to take
# `.0`, so `rain_intensity` validated, range-checked, documented itself in
# `loom describe` and did nothing — with no row in this file able to tell.
# The zero at tick 1800 is not a bug: `dread` is 0.326 there and the deck is
# broken at 0.38 cover, so this fixed point is under a gap. It rains, elsewhere.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 3000 --hold move_z=1 \
  --assert "wind@3,3,-200.speed > 8.0" \
  --assert "rain@3,3,-200.rate > 20.0" >/dev/null

# The cast-and-hook prefix of `DEMO_FIGHT`, which is defined two hundred lines
# below where the trip tapes live. Named here because the creel rows above need
# a hooked fish and nothing more; copying the whole forty-key cadence would be
# the eight-copies mistake the tape block itself was written to end.
DEMO_FIGHT_HEAD="0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:"

# ---------------------------------------------------------------------------
# **THE CREEL OPENS, AND THE WORLD DOES NOT STOP.**
# ---------------------------------------------------------------------------
#
# TAB is the seventh digital channel. The flag lives in `deeper_player.rhai`'s
# own `memory` and crosses to the rules as `creelopen` / `creelshut`, the same
# way `station` and `use` do, because `GameRules` sees no input at all.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 300 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:" \
  --assert "state.creel_open == 1" --assert "events.creelopen == 1" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 320 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 300:bag=1; 301:" \
  --assert "state.creel_open == 0" --assert "events.creelshut == 1" >/dev/null

# **HIS LEGS STOP AND THE ROW ABOVE PROVES IT IS THE CREEL DOING IT.** Same
# tape, W held from tick 0 in both, TAB at 60 in the second. Measured: z is
# **-12.308** with the grid shut and **-3.073** with it open — nine and a
# quarter metres of walk that did not happen — and he is *exactly* -3.073 at
# tick 400 and again at 900, so it is a stop and not a stumble.
#
# **The threshold is -9.0 and the shut figure is -12.308, which is deliberate
# slack.** These two numbers are the boat's walk and they move whenever the
# character controller does: `40bd02d` gave the view mass and the shut figure
# went from -9.310 to -12.308 without this row noticing, which is what the slack
# is for. The claim is "he walked a long way" against "he did not move", and a
# gap of nine metres does not need a tight bound to make it.
#
# The creel takes W/A/S/D because they are the only directional input there is
# and they are also the cursor. **His head is not taken**: the pointer stays
# captured, so mouse-look keeps steering while his hands are in the bag. That
# half cannot be measured here — `loom sim` has no look axis — and it is the
# single most important thing about this system that no gate can see.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 400 \
  --hold "0:move_z=1; 60:move_z=1,bag=1; 61:move_z=1" \
  --assert "Rig/Player.z > -4.0" --assert "state.creel_open == 1" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 400 --hold "0:move_z=1" \
  --assert "Rig/Player.z < -9.0" >/dev/null

# **HANDS ON THE WHEEL.** W/A/S/D are the throttle and the rudder on the mat, so
# TAB there is refused with a word rather than fighting the helm for the keys.
# `at_helm` is `deeper_player.rhai`'s own fact — the wheel is answering you —
# not a rectangle, so it is right for a player who pressed SPACE and let go.
#
# **The word is the half that was missing.** Code 3 is the one refusal a player
# cannot read: every other one is drawn by the open creel's own caption line and
# this one happens with the creel shut, so TAB at the wheel did nothing and said
# nothing. Notice 10 outranks the helm line for two seconds and names the key
# that frees his hands.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 \
  --hold "0:move_z=1; 550:bag=1; 551:" \
  --assert "state.at_helm == 1" --assert "state.creel_open == 0" \
  --assert "state.creel_refused == 3" --assert "events.creelbusy == 1" \
  | grep -q '"message": "HANDS ON THE WHEEL   SPACE lets go, then TAB opens the creel"' 

# **THE CURSOR: ONE TAP IS ONE CELL, A HELD KEY REPEATS TO THE WALL AND STOPS.**
# `move_x`/`move_z` are analogue axes and nothing else in this engine has ever
# edge-detected one, so `deeper_player.rhai` builds the latch, the delay and the
# repeat itself. **Fifteen** ticks of D is one cell and not the eight it could
# have been: fifteen is inside the twenty-tick delay and eight is inside it
# twice over, so a row written at eight passes with the delay set to zero.
# Injected: `CURSOR_DELAY = 0` skids this to two cells. Held to the end of the
# run instead it is 17 `cursor` events and the cursor is at the wall, because
# the grid clamps rather than wraps — deleting the clamp takes it to 17.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 300 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:move_x=1; 275:" \
  --assert "state.creel_cx == 1" --assert "state.creel_cy == 0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 400 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:move_x=1" \
  --assert "state.creel_cx == 2" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 400 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:move_z=-1" \
  --assert "state.creel_cy == 2" --assert "state.creel_cx == 0" >/dev/null

# **AND THE WORLD KEEPS RUNNING, WHICH IS THE HALF THE HUMAN ASKED FOR.**
# `Play::paused` is never touched by any of this. Two rows, and neither needed a
# line of new code — the consequences were already built:
#
#   * Cast, open the creel, and wait. The take still arrives (`events.bite`) and
#     is still missed (`events.spooked`), because LMB turns the item in your
#     hand while the grid is up and cannot also strike a fish.
#   * Hook one, open the creel, and stop reeling. `SLACK_LIMIT` is still
#     counting: `events.escaped` and the phase back to 0. Rummaging mid-fight
#     costs you the fish, and that is the whole argument for not pausing.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 470:bag=1; 471:" \
  --assert "state.creel_open == 1" --assert "events.bite >= 1" \
  --assert "events.spooked >= 1" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1200 \
  --hold "$DEMO_FIGHT_HEAD; 526:bag=1; 527:" \
  --assert "state.creel_open == 1" --assert "events.escaped >= 1" \
  --assert "state.phase == 0" >/dev/null

# **28.0 and not the 30.0 it was.** The berth is a metre further west and the
# helm mat is 0.5 m further aft, so the walk aboard is longer and she is under
# way later; she measured 29.93 at this tick. The claim is "well clear of the
# berth", not the decimal.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_z=1 \
  --assert "Rig/Boat.x > 28.0" --assert "Rig/Player.y > 1.2" \
  --assert "Rig/Boat.y > -0.4" >/dev/null

# **THE TWO COMMANDS THE SCENE FILE ITSELF DOCUMENTS, RUN VERBATIM.** They are
# the only documentation this demo has, they are the first thing a stranger
# types, and neither was in any gate: the headline one shipped as `> 30.0`
# against an actual **29.888** and exited 1 with a wall of hint text. These two
# rows are that file's lines 20 and 26, character for character, so the file
# and the gate cannot drift apart again. The row above asserts the *claim*
# ("well clear of the berth", 28.0 with margin); these assert the *document*.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_z=1 \
  --assert "Rig/Boat.x > 29.0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold "move_z=1,fire=1" \
  --assert "events.bite >= 1" >/dev/null
# And the third number in the same block: the wheel is answering at tick 356,
# and it is not at 350. The file said 240 for three rounds.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 356 --hold move_z=1 \
  --assert "state.at_helm == 1" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 350 --hold move_z=1 \
  --assert "state.at_helm == 0" >/dev/null

# **THE BOARDING LANE, BOTH EDGES.** Round 4 shipped a lane about six degrees
# wide whose taught aim sat on its right-hand edge: `move_x = 0.05` — three
# degrees — boarded nothing, and the two failures were a wall of blue hull and
# a swim, both with "WALK FORWARD TO THE BOAT" still on the HUD. The spawn is
# 0.9 m further west, the boarding treads are 3.0 m wide instead of 2.2, and
# the helm mat is 3.4 m instead of 2.6.
#
# **The committed edges were wrong and one of these rows was sitting on one.**
# Round 5 wrote "-0.18 .. +0.25" here and in the scene, and asserted at +0.20.
# Swept again at 0.01 steps: the wheel is reached from **-0.21 to +0.21** — a
# symmetric **24 degrees**, which is the round's real win and a wider one than
# it claimed on the left. +0.22 does not board, so +0.20 had a hundredth of
# margin and the next scene edit was going to flake it. Both rows are now
# +/-0.18, three hundredths inside a measured edge, and symmetric because the
# lane is.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 --hold "move_z=1,move_x=0.18" \
  --assert "state.at_helm == 1" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 --hold "move_z=1,move_x=-0.18" \
  --assert "state.at_helm == 1" >/dev/null

# **And outside it the caption stops lying.** Both of these used to print the
# walk-forward line for ever. `deeper_player.rhai` states the two facts — it is
# the only script that can, being the only one that sees a key — and packs them
# into the same `station` event the helm already crosses on.
#
# Drift right and you press into her topsides: `along_wish`, not plain speed,
# is what catches that, because a diagonal press keeps 1.6 m/s of sideways
# slide while making no progress at all toward the boat.
#
# **Known limit, stated rather than hidden.** Past about +0.30 the drift is
# shallow enough that he genuinely walks east along the berth railing at more
# than `STUCK_SPEED` and this stays 0. The caption is then unhelpful rather
# than false — the boat is behind his left shoulder and in frame — and the
# threshold that would catch it (1.6 m/s) was measured to make the latch
# flicker, which is worse for a thing an `--assert` reads.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 --hold "move_z=1,move_x=0.26" \
  --assert "state.stuck == 1" --assert "state.aboard == 0" >/dev/null
# Drift left and you walk past her stern into the sea. The slipway is the way
# back and the caption now names it.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 --hold "move_z=1,move_x=-0.22" \
  --assert "state.swimming == 1" --assert "Rig/Player.y < 0.0" >/dev/null

# **Ahead, astern, and both ways round, with the rider still aboard.**
#
# `Player.y > 1.2` on every run is the claim that failed first, and the note
# beside `if grounded { vy = velocity[1]; }` in `deeper_player.rhai` says why:
# a grounded character that keeps accumulating gravity requests a downward move
# every tick, and on a deck that is *also translating* rapier accepts it about
# one tick in seven, so the capsule ratchets into the deck at about 2 cm per
# metre the hull travels and eventually falls out of the bottom of the boat.
#
# `Boat.y > -0.4` is ADR 0063's seven-knot wall, watched rather than assumed:
# thrust is body-frame, so past the ceiling the bow digs in and drives itself
# under. The helm's 1.1 MN measures 2.96 m/s with the trim inside 4 cm.
#
# **Runs 4 and 5 are a mirrored pair on purpose.** One turn assertion passes on
# a hull that always swings the same way; the mirror is what makes it a claim
# about the wheel.
"$LOOM" sim assets/test/rig_drive.loom --ticks 900 \
  --assert "Rig/Boat.x < 3.1" --assert "Rig/Boat.x > 2.9" \
  --assert "Rig/Boat.z < -39.9" --assert "Rig/Boat.z > -40.1" \
  --assert "Rig/Player.y > 1.2" >/dev/null
"$LOOM" sim assets/test/rig_drive.loom --ticks 900 --hold move_z=1 \
  --assert "Rig/Boat.x > 40.0" --assert "Rig/Boat.y > -0.4" \
  --assert "Rig/Player.y > 1.2" >/dev/null
"$LOOM" sim assets/test/rig_drive.loom --ticks 900 --hold move_z=-1 \
  --assert "Rig/Boat.x < -20.0" --assert "Rig/Boat.y > -0.4" \
  --assert "Rig/Player.y > 1.2" >/dev/null
#
# **`state.at_helm == 1` at the end of a turn is the property these two rows
# were missing, and it was false.** They passed on a hull that had already
# acquired its yaw and then coasted: `Input::axis` returns exactly -1, 0 or +1,
# so every A or D a keyboard can press was the full 1.3 MN.m, the hull heeled,
# and the helmsman *slid off the mat* — boat-local x -7.31 to +2.21 in about
# 120 ticks, the length of the aft deck. The throttle then shut and the caption
# read "stand on the mat to steer" at a player who was. `deeper_player.rhai`
# clamps the wheel to 0.3 for it; the numbers that chose 0.3 are in that file.
#
# Straight ahead on the same tape is z = -40.02, so these two are +10.8 and
# -10.8 from it — mirrored to two decimal places, which is what makes the pair
# a claim about the wheel rather than about this hull's handedness.
"$LOOM" sim assets/test/rig_drive.loom --ticks 900 --hold "move_z=1,move_x=1" \
  --assert "Rig/Boat.z > -32.0" --assert "Rig/Boat.z < -26.0" \
  --assert "Rig/Boat.x > 20.0" \
  --assert "Rig/Player.y > 1.2" --assert "state.at_helm == 1" >/dev/null
"$LOOM" sim assets/test/rig_drive.loom --ticks 900 --hold "move_z=1,move_x=-1" \
  --assert "Rig/Boat.z < -48.0" --assert "Rig/Boat.z > -54.0" \
  --assert "Rig/Boat.x > 20.0" \
  --assert "Rig/Player.y > 1.2" --assert "state.at_helm == 1" >/dev/null
# **The long turn, which is the row that actually fails without the clamp.**
# 3,200 ticks of full starboard wheel from the demo's own berth: still at the
# wheel, still making way. Before the clamp this read `at_helm 0` and
# `knots 0.00` from tick 600 onward, and stayed there for ever.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 3200 \
  --hold "0:move_z=1; 400:move_z=1,move_x=1" \
  --assert "state.at_helm == 1" --assert "state.knots > 4.0" >/dev/null

# **Space is hands off the wheel, and run 3 above is its control.**
#
# Standing on the mat used to make `speed` zero unconditionally: all four
# direction keys were the boat and your legs did not exist, so the only exit
# was jump-plus-a-direction and that direction was *simultaneously the wheel*.
# `--hold "jump=1,move_x=1"` used to put the player at y = -0.66 by tick 400
# and -0.700 by tick 600, twelve metres astern of a boat he had just spun.
#
# `hands_off` in `deeper_player.rhai` makes `jump` on the mat let go instead of
# hop. Same key as run 3, plus Space: **-37.66 pinned forever before, -28.56
# after** — he walks off the wheel, across the cockpit, up the boarding steps
# and over her side. (`rig_drive` is thirty metres of open water with no quay
# alongside, so "over her side" ends in the sea, which is what the swim clause
# and the slipway are for. `rig_ashore` below is the same gesture with
# somewhere to land.)
#
# **Run 3 is what says the wheel still exists**: identical `move_z=-1`, no
# Space, twenty-three metres astern. Without that pair, a `hands_off` that
# latched unconditionally would pass this and would have deleted the helm.
"$LOOM" sim assets/test/rig_drive.loom --ticks 300 --hold "jump=1,move_z=-1" \
  --assert "Rig/Player.z > -34.0" >/dev/null

# **Getting off her on foot, with no jump — the seam round 2 left one-way.**
#
# The cockpit sole is 0.64 and the bulwark cap is 1.62, so walking *aboard* is
# a 0.22 m rise the controller steps for free and walking *off* was a 0.98 m
# climb nothing in the game mentioned. `StepLow`/`StepMid`/`StepHigh` make it
# four risers of a quarter of a metre. **No `jump` in run 2**: the defect was
# that leaving required a keypress nothing taught, so a gate that holds `jump`
# cannot fail the way the game failed.
"$LOOM" sim assets/test/rig_ashore.loom --ticks 600 \
  --assert "Rig/Player.y > 1.2" --assert "Rig/Player.y < 1.9" \
  --assert "Rig/Player.z < -8.0" >/dev/null
"$LOOM" sim assets/test/rig_ashore.loom --ticks 120 --hold move_z=-1 \
  --assert "Rig/Player.y > 2.2" --assert "Rig/Player.z > -7.0" >/dev/null

# **Arriving, which is the half of every journey with a wall in it.**
#
# `rig_drive` measures her in open water where nothing is in the way. Hold W
# into the rig and 1.1 MN — a thrust ADR 0063 calls comfortably short of the
# ceiling — submarines her anyway: the bow cannot move, the contact pitches
# her, body-frame thrust follows the bow down. She reached y = -1.46 at tick
# 2400 and was still going, and the player was swimming by 3000, twenty metres
# from a berth with nothing that lets a swimmer climb aboard.
#
# `STALL_TICKS` in `deeper_player.rhai` closes the throttle after three
# quarters of a second of pushing without making way. Run 3 is the control a
# blanket throttle kill would fail; `rig_drive` run 2 is the same control for
# ahead in clear water.
"$LOOM" sim assets/test/rig_bump.loom --ticks 2400 --hold move_z=1 \
  --assert "Rig/Boat.y > -0.4" --assert "Rig/Player.y > 1.2" >/dev/null
"$LOOM" sim assets/test/rig_bump.loom --ticks 3600 --hold move_z=1 \
  --assert "Rig/Boat.y > -0.4" --assert "Rig/Boat.x < -20.0" \
  --assert "Rig/Player.y > 1.2" >/dev/null
"$LOOM" sim assets/test/rig_bump.loom --ticks 900 --hold move_z=-1 \
  --assert "Rig/Boat.x < -48.0" --assert "Rig/Player.y > 1.2" >/dev/null

# **Overboard, and back — the demo's promise that it contains no unrecoverable
# state.** Run 1 is the control that makes run 2 mean anything: without it, a
# run that ended on the deck could have ended there because the character never
# sank at all. `-1.0 < y < 0.0` is floating, not standing, not drowning. This
# pair covers the *east* slipway, which is the only thing it ever covered: it
# starts the swimmer four metres from its foot on the one bearing that works.
"$LOOM" sim assets/test/rig_overboard.loom --ticks 240 \
  --assert "Rig/Player.y > -1.0" --assert "Rig/Player.y < 0.0" >/dev/null
"$LOOM" sim assets/test/rig_overboard.loom --ticks 900 --hold move_x=-1 \
  --assert "Rig/Player.y > 2.2" --assert "Rig/Player.x < 11.5" >/dev/null

# **Miss the boarding lane to port and get back on the rig, and this row is the
# one that can fail.** The row above starts a swimmer four metres from a ramp;
# this one *produces the miss* on the shipped scene with the demo's own taught
# gesture plus twelve degrees of port aim, and then swims from wherever that
# leaves him — measured, (-9.70, -0.70, -26.17), nineteen metres north of the
# deck's own edge.
#
# **Round 5 had no way out of there.** The only ramp was at the east end,
# twenty-six metres away and climbable only heading west; the caption said
# "swim east", and holding east from that point for sixty simulated seconds
# travelled 114 m into open ocean with the same sentence on the screen. A
# stranger who missed by twelve degrees had wedged the demo and would not know
# it. `Rig/Ladder` and the computed bearing in `deeper_rules.rhai` are the fix,
# and this is what says so: one hold, one turn, standing on the deck by 1200.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 --hold "move_z=1,move_x=-0.22" \
  --assert "state.swimming == 1" --assert "Rig/Player.y < 0.0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1200 \
  --hold "0:move_z=1,move_x=-0.22; 600:move_z=-1" \
  --assert "Rig/Player.y > 2.2" --assert "state.swimming == 0" >/dev/null

# ---------------------------------------------------------------------------
# **FISHING, FROM THE BOAT.** The third of the demo's four things, and the
# first round in which the loop and the hub are the same game.
#
# The demo's rules **are** `deeper_rules.rhai`, the
# fight's own file, which grew a hub block that is a no-op in any scene without
# a `Rig/Player`. A scene has one `GameRules` and `play.rs` refuses a second, so
# the alternative was a second copy of two hundred lines of tuned fight
# constants that the five benches above pin and the demo would not — green on
# both sides while the two drift apart.
#
# **Six runs, and they are six different claims.** Two of them are controls and
# neither is decoration: without them a fight that ran unconditionally, or a
# rod that could be cast from the wharf, would pass every row that is left.

# 1. **A keypress reaches the rod.** Walk aboard holding W and the button: the
#    rod casts, a fish bites. It is a *held* button, so `deeper_player.rhai`
#    hands the fight exactly one press and the hook is never set — `spooked` is
#    that same sentence's second half, and it is why this row does not assert a
#    landing. A landing needs taps, and `--hold` is one constant for a run.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold "move_z=1,fire=1" \
  --assert "events.bite >= 1" --assert "events.spooked >= 1" \
  --assert "state.aboard == 1" >/dev/null

# 2. **The control: the rod is aboard-only.** Same button, no walk. He never
#    boards, so no `angler` event is ever emitted and nothing casts. Without
#    this row an emit with the `aboard` guard deleted passes row 1 unchanged.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold "fire=1" \
  --assert "events.bite == 0" --assert "state.aboard == 0" >/dev/null

# 3. **SPACE now changes what is on the screen, in both directions.** Round 3
#    shipped a hands-off latch whose only observable consequence was that the
#    boat stopped, while the caption went on saying "W ahead" — because the
#    rules script computed its own `at_helm` from a rectangle and
#    `deeper_player.rhai` computed a different one from the wheel, and the two
#    were pronounced the same. There is one now, it is stated by the script
#    that sees the key, and it crosses as an event on change. Same hold as the
#    `rig_drive` pair above, plus Space: aboard, and *not* driving.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold "move_z=1,jump=1" \
  --assert "state.aboard == 1" --assert "state.at_helm == 0" >/dev/null

# 4. **A fish is hooked, played and landed, in the demo's own rules.** The
#    fight cannot be driven by `--hold` — it wants correctly-timed taps for
#    twenty seconds — so `rig_fish.loom` names a reference angler the way the
#    five benches do. See that file's header for why a pilot is right here and
#    refused in `rig_drive.loom`.
"$LOOM" sim assets/test/rig_fish.loom --ticks 1500 \
  --assert "events.hooked >= 1" --assert "events.landed >= 1" \
  --assert "state.fish >= 1" >/dev/null

# 5. **A hub is not a bench.** `Play::run` returns the instant `status` is won
#    or lost — the whole simulation stops — which is right for the benches and
#    would make catching a fish the end of the demo. In a hub the fight never
#    latches a terminal status and the terminal phases time out back to the
#    cast. By 1800 the pilot is playing his second fish.
#    **And the E press is what lets the second one start.** `phase` now stays at
#    5 while a fish is on the line, so an unattended rod never casts again --
#    which is the point of it and is also why this row carries the same press
#    5b does.
"$LOOM" sim assets/test/rig_fish.loom --ticks 1800 --hold "1200:interact=1; 1201:" \
  --assert "status == playing" --assert "state.fish >= 1" \
  --assert "events.hooked >= 2" >/dev/null

# 5b. **THE FOUR SLOTS, ALL FOUR RULES, IN ONE RUN.** Round 4's hold was four
#     supplies in four slots that nothing ever spent, so `hold.len() < SLOTS`
#     could not be false and no item changed a number — a checklist, and a
#     fifth checkbox would not have converted it. `rig_fish` carries three
#     supplies within reach of the angler (see its header for why not four) and
#     this row is the whole loop: he takes them (`pickup`), **the fish eats the
#     bait at the take** (`spend`), and the catch **takes the slot the bait left
#     free** — which is what `state.infish` says and `state.carried == 4` makes
#     a full hold rather than a coincidence.
#
#     **It used to assert `events.stow` and that row could not fail.** The fish
#     box was drawn on the bait box at boat-local (-7.80, 0.0), 0.90 m from the
#     nearest point of the helm rectangle against a `RANGE` of 2.0 — so a landed
#     fish was stowed on the tick it was landed, `notice` went 2 -> 3 in one
#     tick, and `state.infish` was 0 at every sample anyone took. The capacity
#     rule the whole of round 5 was built around held a slot for a single frame.
#     The box has moved to starboard, 2.32 m from the wheel, and stowing is now
#     a walk — which is asserted where a walk can happen, in `rig_trip.loom`
#     below. What is left here is the half this scene can see: that the fish is
#     **carried**, for 455 ticks and counting.
#
#     **And it is carried because a key was pressed.** The landing at 1147 now
#     puts her on the *line* (`state.online`), and the one `--hold` segment in
#     this otherwise pilot-driven scene is the E that takes her off it. Without
#     that press `infish` is 0 and `carried` is 3 for the whole run, which is
#     the shape of this row's own falsification.
"$LOOM" sim assets/test/rig_fish.loom --ticks 1800 --hold "1200:interact=1; 1201:" \
  --assert "events.pickup >= 3" --assert "events.spend >= 1" \
  --assert "events.take >= 1" \
  --assert "state.infish >= 1" --assert "state.carried == 4" >/dev/null

# 5b2. **TWO FISH IN THE CREEL AT ONCE, WHICH FOUR SLOTS COULD NOT HOLD.**
#
#      **This row used to assert the opposite and the change is the point.** In
#      four slots, four supplies and a fish was five things, so `rig_fish`'s
#      second catch at 2600 was refused and the row pinned the refusal. Nine
#      cells and two sprats of one cell each is five cells of nine, so it now
#      goes in — and the *shape* is what says so: `eab/cd./...` names where all
#      five landed, and `.` for the four that are free.
#
#      The refusal has not been dropped, it has moved to where it is *spatial*
#      rather than arithmetic — 5b3 below, which lands a conger on the demo's
#      own taught tape and cannot fit her beside three curios in nine cells.
"$LOOM" sim assets/test/rig_fish.loom --ticks 2700 \
  --hold "1200:interact=1; 1201:; 2600:interact=1; 2601:" \
  --assert "state.infish == 2" --assert "state.online == 0" \
  --assert "state.creel_free == 4" --assert "state.creel_drift == 0" \
  | grep -q '"creel_cells": "eab/cd\./\.\.\."'

#      **And the way out is the box, which takes a catch off the line as well as
#      out of your hands.** That is why the E ladder tests the box *before* the
#      line: if the only route below ran through his hands, a full hold would be
#      a fish he could neither hold nor stow and no way to free a slot until he
#      sailed home. SPACE for his legs, the dogleg round the bait box, one E at
#      1.0 m — and **both** fish go below in the one press. `stowed` is 2,
#      `carried` falls to 3, and the rod is free again (`phase` back to 1).
#      `carried` is **2** and not the 3 it was, and the two are a different
#      trajectory rather than a lost item: the creel took the second fish
#      instead of refusing it, so the pilot cast again, and the bite before 2730
#      ate the bait he had picked back up. `creel_cells` is `.ab/.../...` — the
#      two curios, and seven empty cells where two fish were.
"$LOOM" sim assets/test/rig_fish.loom --ticks 2730 \
  --hold "1200:interact=1; 1201:; 2600:interact=1; 2601:; 2620:jump=1; \
2630:move_x=-1; 2680:move_z=-1; 2720:move_z=-1,interact=1; 2721:move_z=-1" \
  --assert "state.stowed == 2" --assert "state.online == 0" \
  --assert "state.infish == 0" --assert "state.carried == 2" \
  --assert "state.creel_free == 7" --assert "state.full == 0" >/dev/null

# 5c. **And bait is the reason it is an inventory and not a checklist.** The
#     control is the same scene with nothing in the hold: `fight_skilled.loom`
#     is not a hub, so `state.bait` never exists there and the unbaited
#     `BITE_MIN`/`BITE_SPAN` the five benches pin are untouched by any of this.
#     Here the demo is walked aboard holding the button: it picks up the bait
#     on the way, casts, and the take spends it.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold "move_z=1,fire=1" \
  --assert "events.spend >= 1" --assert "state.bait == 0" \
  --assert "state.carried == 3" >/dev/null

# ---------------------------------------------------------------------------
# **THE FOUR TAPES THE DEMO'S OWN ROWS ARE BUILT FROM.**
#
# Eight rows below used to carry a verbatim copy of the same forty-key fishing
# cadence, and this round had to edit every one of them: the catch is taken off
# the rod, stowed and delivered on **E** now rather than by standing near a box,
# so three presses went into each. Eight copies of one tape is eight chances to
# fix seven of them.
#
# They nest, and each is one more leg of the trip:
#
#   DEMO_FIGHT  W aboard, SPACE off the mat, CLICK to cast, CLICK to hook, then
#               SHIFT in a 40-on / 50-off cadence. **The fish is landed at 1830
#               and is on the LINE, not in the hold** -- that is the round's
#               change and `state.online` is the number for it.
#   DEMO_BOX    + E at 1900 (she comes off the line into his hands), the A/S
#               dogleg round the bait box, and E at 2000 at the fish box. Ends
#               standing at the box with `stowed == 1`.
#   DEMO_HELM   + the reverse dogleg back onto the helm mat. Ends under way.
#   DEMO_HOME   + astern to the berth, SPACE, over the boarding steps, west
#               along the wharf, and **E at 3740 at the crate**.
#
# **Why 3740 and not "when he arrives".** He is never inside `RANGE` of the
# crate while he is wedged against the bulwark at (-7.82, -8.33) -- that is
# 2.44 m, and the row that used to pass here was passing on a *later* pass:
# measured, the walk from 3700 to 3800 crosses within 1.27 m of it at about
# 3744. 3730 through 3760 all deliver, so the press is in the middle of a
# thirty-tick window rather than on its edge.
DEMO_FIGHT="0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1"
DEMO_BOX="$DEMO_FIGHT; 1900:move_x=-1,interact=1; 1901:move_x=-1; \
1950:move_z=-1; 2000:move_z=-1,interact=1; 2001:move_z=-1"
DEMO_HELM="$DEMO_FIGHT; 1900:move_x=-1,interact=1; 1901:move_x=-1; \
1950:move_z=-1; 2000:move_z=1,interact=1; 2001:move_z=1; 2035:move_x=1; 2060:; \
2100:move_z=1"
DEMO_HOME="$DEMO_HELM; 2200:move_z=-1; 2600:; 2660:jump=1; 2670:move_z=-1; \
2750:move_x=-1,move_z=-0.15; 3550:move_z=-1,move_x=-0.3; \
3740:move_z=-1,move_x=-0.3,interact=1; 3741:move_z=-1,move_x=-0.3"

# 5d. **THE PLAYER FISHES, WITH HIS OWN HANDS, AND NOTHING HERE HAD EVER DONE
#     THAT.** Every landing in this file until now was `Rig/Pilot/skilled` —
#     `rig_fish` and `rig_trip` both hand the rod to a pilot script and drive
#     only the walking with `--hold`. The `rig_trip` header says so honestly,
#     and the justification it gives ("a landing needs taps, and `--hold` is one
#     constant for a run") **stopped being true in the commit that wrote it**:
#     `--hold` became a schedule that round. So the tool to close this existed
#     and was not pointed at the gap, and the demo's headline claim — you can
#     fish — was a claim about a robot.
#
#     The tape, in the order a player would press it: W aboard, SPACE off the
#     mat at 420 so his legs come back, CLICK at 460 to cast, CLICK again at
#     525 inside the take window, then SHIFT in a **40-on / 50-off** cadence to
#     the landing at 1880.
#
#     **`off > on` is the whole skill, and the two controls below are what make
#     that a claim.** Forty-nine duty cycles were swept: ten land the fish and
#     every single winner has more release than pull. A cadence is a decision a
#     player learns by feel; if either control ever stops failing, the fight has
#     become a slot pull and this says so unattended.
#     **The landing is no longer the end of it, and that is this round.** A fish
#     that came over the rail used to arrive in a slot on the same tick; it now
#     hangs off the rod tip (`state.online`) until the player presses E. So this
#     row is two claims: the fight can be played from the keyboard, and the
#     catch is taken off the line by a key rather than by the clock.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1880 --hold "$DEMO_FIGHT" \
  --assert "events.hooked >= 1" --assert "events.landed >= 1" \
  --assert "state.online == 1" --assert "state.infish == 0" \
  --assert "state.carried == 3" --assert "state.fishsize == 6" \
  | grep -q '"message": "SHE IS ON   press E to take her off the line"'

# 5b3. **AND SHE DOES NOT FIT, WHICH IS THE WHOLE CREEL IN ONE ROW.**
#
#      **`state.infish == 1` used to be the assertion here and it has become
#      `state.creel_hand == 1`.** The demo's own taught tape lands a **conger**
#      — `fishsize == 6`, asserted one row up, and the size is rolled on the
#      bite from a stream of its own so that `landed_tick` and `fight_ticks`
#      below do not move. Six cells, and the creel holds three curios in
#      `a.b/c../...`:
#
#          a . b        six cells free, and NOT a 2x3 anywhere. The hole at
#          c . .        (1,0) is where the bait was before the fish ate it —
#          . . .        fragmentation, arriving for free, from the loop.
#
#      So E is refused and she **stays on the line**, which is not a
#      punishment: `phase` only resets when `online == 0`, so a landed fish
#      hangs off the rod tip indefinitely — traced to tick 3900, two thousand
#      after the landing, still there. The sentence on screen names her size and
#      both ways out.
#
#      **The first version of this put her in his HAND and that was a dead
#      end.** You have one hand: with the conger in it you cannot lift a curio
#      to repack and you cannot ditch one either, so the only move left inside
#      the creel is to throw away the fish you just fought for. Waiting on the
#      line leaves both the hand and the ditch free, which is what makes the
#      three rows below possible at all.
#
#      This is the row that would still pass if the creel were a counter, so it
#      also greps the shape. There is exactly one right string.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1910 \
  --hold "$DEMO_FIGHT; 1900:interact=1; 1901:" \
  --assert "events.refused >= 1" --assert "state.online == 1" \
  --assert "state.infish == 0" --assert "state.creel_hand == 0" \
  --assert "state.creel_free == 6" --assert "state.carried == 3" \
  | grep -q '"creel_cells": "a\.b/c\.\./\.\.\."'

#      **And the hatch is the way out that needs no packing at all.**
#      `DEMO_BOX` is the same tape plus the dogleg round the bait box and one E
#      at the fish box: she comes off the line and goes below in that one press.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2010 --hold "$DEMO_BOX" \
  --assert "state.stowed == 1" --assert "state.online == 0" \
  --assert "events.stow >= 1" >/dev/null

# 5b4. **THE WHOLE SYSTEM, AS ONE THING A PLAYER DOES.** This is the row the
#      creel exists for and it is the only one that uses every part of it.
#
#      He has fought a conger for twenty-two seconds; she is over the rail and
#      will not go in the bag. The creel reads:
#
#          a . b        FLSK (0,0), LINE (2,0), LAMP (0,1). Six cells free.
#          c . .        The hole at (1,0) is where the bait was before the fish
#          . . .        ate it. Six free cells and no 2x3 anywhere in them.
#
#      TAB. Two taps of D to walk the cursor to the line spool at (2,0) — two
#      taps and not a hold, because the repeat delay is twenty ticks. Hold SHIFT
#      for a second and the spool goes over the side, which opens columns 1 and
#      2 across all three rows. TAB again, E at the rod, and she goes in the way
#      she was born — **unturned**, 2 wide and 3 tall: `acc/bcc/.cc`, one cell
#      left in the creel. 5b9 is the same puzzle solved the other way, and that
#      one turns her.
#
#      **Every claim in that paragraph is a `--assert` or a `grep` below**, and
#      the two strings are the ones a stub cannot produce without having
#      implemented the packer, the cursor, the ditch and the placement.
DEMO_CONGER="$DEMO_FIGHT; 1900:interact=1; 1901:; 1910:bag=1; 1911:; \
1930:move_x=1; 1938:; 1960:move_x=1; 1968:; 1990:sprint=1; 2060:; \
2080:bag=1; 2081:; 2100:interact=1; 2101:"

"$LOOM" sim assets/games/deeper_demo.loom --ticks 1980 --hold "$DEMO_CONGER" \
  --assert "state.creel_open == 1" --assert "state.creel_cx == 2" \
  --assert "state.creel_cy == 0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2070 --hold "$DEMO_CONGER" \
  --assert "events.ditched == 1" --assert "state.line == 0" \
  --assert "state.creel_used == 2" \
  | grep -q '"creel_cells": "a\.\./b\.\./\.\.\."'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2110 --hold "$DEMO_CONGER" \
  --assert "state.creel_open == 0" --assert "state.online == 0" \
  --assert "state.infish == 1" --assert "state.creel_free == 1" \
  --assert "state.creel_drift == 0" \
  | grep -q '"creel_cells": "acc/bcc/\.cc"'

# 5b5. **LIFT, PLACE, AND THE TWO REFUSALS.** The same three keys on a quiet
#      deck, where the shapes are all one cell and only the rules are under
#      test. E lifts what the cursor is over; E on a free cell puts it down; E
#      on a taken one is refused with code 1; SHIFT on nothing is refused with
#      code 2. Lifting frees the cells on the same tick, because occupancy is
#      folded from the placement list and removing the placement *is* freeing
#      them — `creel_drift` is what would catch that going wrong.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 400 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:" \
  --assert "state.creel_hand == 1" --assert "state.creel_used == 3" \
  --assert "events.lift == 1" --assert "state.creel_drift == 0" \
  | grep -q '"creel_hand_label": "FLSK"'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 400 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
300:interact=1; 301:" \
  --assert "state.creel_hand == 0" --assert "events.place == 1" \
  --assert "state.creel_used == 4" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 400 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
280:move_x=1; 288:; 300:interact=1; 301:" \
  --assert "state.creel_hand == 1" --assert "state.creel_refused == 1" \
  --assert "events.place == 0" >/dev/null
#      The empty cell is (0,2) and it takes two taps of S to reach: the four
#      supplies land in `abc/d../...`, so every cell in the top row and the
#      first of the second are taken. A row aimed at (1,0) throws the BAIT away
#      and reports a clean pass, which is what the first draft of it did.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 420 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:move_z=-1; 268:; \
280:move_z=-1; 288:; 300:sprint=1; 370:" \
  --assert "state.creel_cy == 2" --assert "state.creel_refused == 2" \
  --assert "events.ditched == 0" >/dev/null

# 5b8. **THE TEN NAMES THE OVERLAY READS, ALL EXPORTED BY THE GAME.**
#
#      `hud::creel` is drawn only in `loom run`, so **no gate in this project
#      can see it** — not `cargo xtask image`, which has no window and never
#      constructs a `Ui`, and not `loom sim`, which draws nothing. Its eleven
#      unit tests prove the layout and the shapes against a headless
#      `egui::Context`, and they would all still pass if the rules script
#      renamed `creel_cx` tomorrow: `Creel::read` would return `None`, the grid
#      would vanish, and nothing anywhere would say so.
#
#      This row is the seam between the two halves. Every name the overlay
#      reads, checked against one real run of the real game. It is the cheapest
#      possible test and it is the only detector this feature's drawing has.
#
#      Keep it in step with `state.number(...)` / `state.text(...)` in
#      `hud.rs`'s `Creel::read`.
#      **One run, ten greps**, not ten runs: a `deeper_demo` tick is about
#      1.3 ms and this block is already 135 `loom sim` invocations long.
creel_state=$("$LOOM" sim assets/games/deeper_demo.loom --ticks 400 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:")
for name in creel_cells creel_kinds creel_open creel_cx creel_cy \
            creel_hand creel_hand_label creel_hand_w creel_hand_h creel_fits; do
  printf '%s\n' "$creel_state" | grep -q "\"$name\":" || {
    echo "green: the creel overlay reads state.$name and the game does not export it" >&2
    exit 1
  }
done
unset creel_state

# 5b6. **THE DITCH NEEDS THE KEY HELD, AND A SECOND IS A SECOND.** Twenty ticks
#      of SHIFT throws nothing away; seventy does. It is the only irreversible
#      act in this game, which is why it is a held key — and why it is not
#      SPACE, which on the helm mat is the release and where a fat finger is a
#      boat adrift.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 420 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
300:sprint=1; 320:" \
  --assert "events.ditched == 0" --assert "state.creel_hand == 1" \
  --assert "state.creel_ditching == 0" >/dev/null
#      **And the bar he is holding the key against, which was on screen for one
#      tick in sixty.** `deeper_player.rhai` emits `ditching` on *change* — four
#      events a ditch, not sixty — and the rules script read it into a per-tick
#      local, so `OVER THE SIDE ||` appeared on tick 314 and was gone again at
#      315. A second of the only irreversible key in the game, against a caption
#      reading `HOLDING FLSK`. It is latched now, and these are the only two
#      rows in the block that stop with a key still down.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 320 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
300:sprint=1" \
  --assert "state.creel_ditching == 1" --assert "events.ditched == 0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 350 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
300:sprint=1" \
  --assert "state.creel_ditching == 3" --assert "events.ditched == 0" >/dev/null
#      **And it stops at four, and stops being a bar at all.** `ditch_held`
#      keeps counting while the key is down: uncapped, the quarter reached 6 at
#      a second and a half and drew `let go of SHIFT to keep it` over a hand
#      that had been empty since 60. Held to tick 420 without ever letting go,
#      the bar is nought and the line names what went over.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 420 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
300:sprint=1" \
  --assert "state.creel_ditching == 0" --assert "events.ditched == 1" \
  | grep -q '"message": "OVER THE SIDE   the thermos is gone"' 
#      **And notice 11 could not reach the screen while the creel was open at
#      all** — the open grid owns the caption line and never consulted the
#      notices, so a bar that had been filling for a second vanished into the
#      default help string with no word about what had gone over the side. It
#      is above `HOLDING` and below the refusals now.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 420 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
300:sprint=1; 370:" \
  --assert "events.ditched == 1" --assert "state.creel_hand == 0" \
  --assert "state.thermos == 0" --assert "state.creel_used == 3" \
  | grep -q '"message": "OVER THE SIDE   the thermos is gone"' 

# 5b7. **SHUTTING IT PUTS WHAT IS IN YOUR HAND BACK.** A held item has no
#      picture and no verb outside the grid, so walking away with one is state
#      the player cannot see. Refusing to close was the other candidate and is
#      worse: it makes TAB stop working, which reads as a bug rather than a
#      rule. It always fits — it came out of this grid one press ago and its own
#      cells are still free.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 400 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
300:bag=1; 301:" \
  --assert "state.creel_hand == 0" --assert "state.creel_used == 4" \
  --assert "state.thermos == 1" --assert "events.stash == 1" >/dev/null

# 5b9. **THE OTHER SOLVE, AND IT IS THE ONE THAT TURNS HER.** 5b4 ditches the
#      LINE at (2,0) and the conger goes in **unturned**, `acc/bcc/.cc`. That is
#      not the only answer to the same puzzle and — until this row — it was the
#      only one any gate had ever seen. Rotation could be deleted from the
#      packer outright and the whole block still exited 0.
#
#      From the same refused state, `a.b/c../...`:
#
#          a . b        one tap of S puts the cursor on the LAMP at (0,1),
#          c . .        SHIFT sends it over the side, and rows 1 and 2 are a
#          . . .        clean three-wide-two-tall hole.
#
#      A 2x3 conger still does not fit anywhere in it — x = 0 is blocked by the
#      FLSK and x = 1 by the LINE. **Turned she is 3x2 and she drops straight
#      in**, filling both rows: `a.b/ccc/ccc`. `first_fit` finds that on its
#      own, which is the rot = 1 branch of `cells_of` under test at last.
#
#      It is also a press cheaper than 5b4 — one tap of S against two of D — so
#      the solve that needs the rotation is the better one, which is the whole
#      argument for the mechanic being there.
#
#      **And the third curio is the wrong answer**, which is the sentence the
#      grid earned and the row that says the puzzle is a puzzle. The FLSK at
#      (0,0) is the one already under the cursor and so the cheapest of the
#      three to throw away — zero taps — and it leaves `..a/b../...`, six free
#      cells in an L with no 2x3 and no 3x2 in them. She stays on the line.
#      Measured all three ways: FLSK refused, LAMP turned, LINE unturned.
DEMO_TURNED="$DEMO_FIGHT; 1900:interact=1; 1901:; 1910:bag=1; 1911:; \
1930:move_z=-1; 1945:; 1960:sprint=1; 2030:; 2050:bag=1; 2051:; 2100:interact=1; 2101:"
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2110 \
  --hold "$DEMO_FIGHT; 1900:interact=1; 1901:; 1910:bag=1; 1911:; \
1930:sprint=1; 2000:; 2050:bag=1; 2051:; 2090:interact=1; 2091:" \
  --assert "events.ditched == 1" --assert "state.thermos == 0" \
  --assert "state.online == 1" --assert "state.infish == 0" \
  | grep -q '"creel_cells": "\.\.a/b\.\./\.\.\."'

"$LOOM" sim assets/games/deeper_demo.loom --ticks 2110 --hold "$DEMO_TURNED" \
  --assert "events.ditched == 1" --assert "state.lantern == 0" \
  --assert "state.online == 0" --assert "state.infish == 1" \
  --assert "state.creel_free == 1" --assert "state.creel_drift == 0" \
  | grep -q '"creel_cells": "a\.b/ccc/ccc"'

#      **And LMB, which is the only key in the creel with no coverage at all.**
#      Lift the turned conger back out — she keeps the way up she was packed,
#      3 wide and 2 tall, and `creel_fits` says yes where she came from. One
#      click and she is 2x3 again and `creel_fits` says no from the same cell,
#      which is the readout the overlay paints green or red. Press E there and
#      it is refused with code 1 rather than silently ignored.
DEMO_TURN_VERB="$DEMO_TURNED; 2120:bag=1; 2121:; 2140:interact=1; 2141:; \
2160:fire=1; 2161:; 2180:interact=1; 2181:"
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2190 --hold "$DEMO_TURN_VERB" \
  --assert "events.turn == 1" --assert "state.creel_hand_w == 2" \
  --assert "state.creel_hand_h == 3" --assert "state.creel_fits == 0" \
  --assert "state.creel_refused == 1" --assert "events.place == 0" >/dev/null

#      Click again and she is back to 3x2, E puts her down, and the picture is
#      the one 5b9 started from. A turn is reversible; that is why it is a tap
#      and the ditch is a held key.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2240 \
  --hold "$DEMO_TURN_VERB; 2200:fire=1; 2201:; 2220:interact=1; 2221:" \
  --assert "events.turn == 2" --assert "events.place == 1" \
  --assert "state.creel_hand == 0" --assert "state.creel_drift == 0" \
  | grep -q '"creel_cells": "a\.b/ccc/ccc"'

# 5b10. **THE REFUSAL EXPIRES WHEN THE CURSOR LEAVES THE CELL IT WAS ABOUT.**
#
#      `IT WILL NOT GO THERE   LMB turns it — or move the cursor` tells the
#      player to do a thing, and until this row doing it changed nothing: he
#      walked the item to a cell where the overlay was painting its footprint
#      **green** and the caption was still telling him it would not fit. Two
#      readouts of one fact, disagreeing, and the wrong one made of words.
#
#      Lift the FLSK out of (0,0), step onto the BAIT at (1,0), press E: code 1
#      and `creel_fits == 0`. Step down to (1,1), which is empty: code 0 and
#      `creel_fits == 1`. Every refusal is a fact about the cell under the
#      cursor, so all of them expire together when it moves.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 320 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
280:move_x=1; 288:; 300:interact=1; 301:" \
  --assert "state.creel_refused == 1" --assert "state.creel_fits == 0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 360 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
280:move_x=1; 288:; 300:interact=1; 301:; 320:move_z=-1; 328:" \
  --assert "state.creel_refused == 0" --assert "state.creel_fits == 1" \
  --assert "state.creel_cx == 1" --assert "state.creel_cy == 1" >/dev/null

#      **And a step into the wall is not a step.** The clear is tested after the
#      clamp, so D at the right-hand column moves nothing and the message
#      stands. A message that blinks off when nothing moved is the same bug the
#      other way round, and doing this before the clamp is how you get it.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 380 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
280:move_x=1; 288:; 300:move_x=1; 308:; 320:interact=1; 321:; \
340:move_x=1; 348:" \
  --assert "state.creel_cx == 2" --assert "state.creel_refused == 1" >/dev/null

#      **And code 5, which had no row either.** LMB on a one-cell item says so
#      rather than doing nothing, because "I pressed it and nothing happened" is
#      the complaint every silent no-op earns. The sprat was `rot: 1` in the
#      item table until this row was written — so LMB on a one-cell *fish*
#      reported success and changed nothing, which is the no-op the message
#      exists to prevent, wearing the message's own clothes.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 300 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
280:fire=1; 281:" \
  --assert "state.creel_refused == 5" --assert "state.creel_hand == 1" \
  | grep -q '"message": "IT IS ONE CELL   turning it would change nothing"'

#      **Code 4, which exists because code 2 was lying.** LMB with an empty hand
#      used to answer `NOTHING IN THAT CELL`. The cursor is on the FLSK: there
#      is something in that cell, and the player who reached for the turn key
#      before the lift key was told otherwise. It now names what is missing and
#      the key that fixes it.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 300 \
  --hold "0:move_z=1; 240:; 250:bag=1; 251:; 260:fire=1; 261:" \
  --assert "state.creel_refused == 4" --assert "state.creel_hand == 0" \
  --assert "events.turn == 1" \
  | grep -q '"message": "NOTHING IN YOUR HAND   E lifts it out first, then LMB turns it"'

# 5d2. **YOU CAN CAST FROM ANYWHERE ABOARD, AND NOTHING SAID SO.** The engine
#      has gated the cast on `aboard` rather than on a station since the day it
#      shipped — `deeper_player.rhai` emits `angler` from anywhere on the deck —
#      and the rod stood in a rack on the aft port bulwark, so the player was
#      pressing CLICK at a piece of scenery across the boat and had no reason to
#      believe he could walk away from it. The rod is in his hands now
#      (`rod_hold.rhai`), which is the half a gate cannot see, and this row is
#      the half it can: SPACE for his legs, ninety ticks of D up her starboard
#      side, and then CLICK. He is at boat-local x **+1.77**, nine metres from
#      where the rack used to be, and the line goes out.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 610 \
  --hold "0:move_z=1; 420:jump=1; 430:move_x=1; 560:; 600:fire=1; 606:" \
  --assert "state.phase == 1" --assert "state.plx > 1.0" \
  | grep -q '"message": "LINE OUT, BAITED   a take is close"'

#      **And the control, which is the other half of "anywhere aboard".** Hold
#      the trigger from the spawn without ever boarding: no `angler` event is
#      ever emitted, so a rod cast from the wharf is not a refusal the fight has
#      to carry — it is a thing that never reaches it. `phase` is still 0 after
#      five seconds of holding it down.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 300 --hold "0:fire=1" \
  --assert "state.phase == 0" --assert "events.angler == 0" >/dev/null

# 5e. **The two endings a first-timer actually gets, and they used to be
#     lowercase bench strings.** `the line snapped` and `it threw the hook`
#     came from `fishing.loom`, a bench whose reader was an assertion; on a HUD
#     they read as debug prints, and neither said that the bait — spent at the
#     bite — was gone. Every *other* ending in `deeper_rules.rhai` goes through
#     `notice` and comes out in caps with a next step.
#
#     **Asserted with `grep`, because `--assert` has no `message` axis** — it
#     reads `status`, `state.<name>` and `events.<kind>` and nothing else. The
#     leading capital is the assertion: it is what separates a sentence written
#     for a player from one written for a test harness, and it is exactly what
#     regressed last time.
#
#     Same tape as 5d up to the hook, then: hold SHIFT flat out (the thing the
#     screen offers, so the thing a stranger does) and never touch it at all.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; 540:sprint=1" \
  --assert "events.snap >= 1" --assert "state.bait == 0" \
  | grep -q '"message": "THE LINE SNAPPED'
#     **900, not 1400, and this row has never once passed.** It shipped with
#     the round that wrote it and nobody re-ran the block: a hub times a
#     finished fight out after `OVER_TICKS` (180) and hands the line back to the
#     situation, so the hook is thrown at tick 845, the sentence is on screen
#     until 1025, and at 1400 the caption reads `ABOARD   CLICK to cast`. The
#     `--assert`s either side of the pipe both passed at 1400, which is what hid
#     it: `events.escaped` is cumulative and `state.bait` had stayed spent. Only
#     the `grep` could see it, and only because a message is a thing with a
#     lifetime and an assertion is not. Verified against `git show HEAD` of all
#     four demo files, so it is this row and not this round.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:" \
  --assert "events.escaped >= 1" --assert "state.bait == 0" \
  | grep -q '"message": "IT THREW THE HOOK'

# 5f. **The float and the fish, which are the only things in the player's own
#     frame that say any of the above is happening.** Rendered from
#     `Rig/Player/Eye` at eight headings, the whole fishing loop used to be one
#     unchanging picture of two rods and a horizon: the cast, the take, the run
#     and the landing were four different *strings* over the same frame. Round
#     7 gave them geometry — `float_bob.rhai` and `fish_hang.rhai`, two node
#     scripts writing their own transforms off `state`.
#
#     **Asserted as positions, not as pixels, and that is the cheap half being
#     the right half.** `local_y` is the node's own transform in the boat's
#     frame, which is exactly what the script wrote, so these rows cost nothing
#     and fail on a deleted node, a renamed node, a thrown script and a drifted
#     phase number alike. What they cannot see is whether the thing is *visible*
#     — that was checked by rendering it and looking, and the renders are named
#     in the round's report.
#
#     The float's three states, on the same tape: stowed at the rod at 1.500,
#     on the water at -0.057, and under it at -0.461 during the six hundred
#     milliseconds the take is answerable in. The dunk **is** the tell; before
#     it, `BITE!  CLICK NOW` was the entire signal and it was text.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 440 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:" \
  --assert "state.phase == 0" --assert "Rig/Boat/Float.local_y > 1.4" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 500 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:" \
  --assert "state.phase == 1" --assert "Rig/Boat/Float.local_y < 0.0" \
  --assert "Rig/Boat/Float.local_y > -0.2" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 530 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:" \
  --assert "state.phase == 3" --assert "Rig/Boat/Float.local_y < -0.3" >/dev/null
#     And the catch: 1.560 clipped to the rod when there is none, 1.800 hanging
#     in the cockpit when there is one. **`state.fishsize` is the condition now
#     and it was `state.infish`** — `infish` used to mean "in your hands" and
#     since the creel it means "packed in the bag", so leaving this on it drew a
#     fish hanging in the cockpit that the player had already put away.
#     `fishsize` is the cell area of the fish he can *see*: on the rod tip, or
#     in his hands, and zero for one that is in the creel.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 500 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:" \
  --assert "state.fishsize == 0" --assert "Rig/Boat/Catch.local_y < 1.6" >/dev/null
#     **Three poses now, not two, and the middle one is the round's change.** On
#     the line she hangs off the rod *tip* at boat-local y 2.883; taken off, in
#     his hands, at 1.800; parked at the rod butt at 1.560. The tip pose is the
#     only thing on screen that says a landed fish is still attached to
#     something, which is the whole reason E has to be pressed.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1880 --hold "$DEMO_FIGHT" \
  --assert "state.online == 1" --assert "Rig/Boat/Catch.local_y > 2.6" >/dev/null
#     **And she is as big as her footprint.** `scale` is `6.5 * (1 + 0.28 *
#     (area - 1))`, so the conger this tape lands renders at 1.56 m against a
#     sprat's 0.65 — and the in-hand pose backs off a tenth of a metre per extra
#     cell to keep her out of the lens. `local_y` is untouched by both, which is
#     what these rows still pin.
#
#     **A fish in the CREEL is not drawn at all**, which is the third pose and
#     the round's change: 1.560, parked at the rod butt, because a fish you have
#     packed away is packed away. So "in his hands" is reachable only by lifting
#     her back out of a cell with the grid open — `$DEMO_CONGER` plus a TAB, a
#     tap of D and an E — and that is what the second row here does. 1.803.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2110 --hold "$DEMO_CONGER" \
  --assert "state.infish == 1" --assert "state.fishsize == 0" \
  --assert "Rig/Boat/Catch.local_y < 1.6" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2180 \
  --hold "$DEMO_CONGER; 2120:bag=1; 2121:; 2140:move_x=1; 2148:; \
2170:interact=1; 2171:" \
  --assert "state.creel_hand == 1" --assert "state.fishsize == 6" \
  --assert "Rig/Boat/Catch.local_y > 1.7" \
  --assert "Rig/Boat/Catch.local_y < 2.2" >/dev/null
#     **And it is in his hands, which for eight rounds it was not.** The hang was
#     a fixed boat-local `(-7.40, 1.85, -3.60)`: he walked to the fish box and it
#     stayed exactly where it was, over open water, with the caption reading
#     `FISH IN HAND`. Measured at tick 1910 — `Catch` at world (-0.800, 1.878,
#     -14.056), `Player` at (-0.746, 1.607, -12.305), and the gap grows for the
#     whole walk. It now rides `state.plx/ply/plz`, which `deeper_rules.rhai`
#     solves in her frame from two of her own nodes. Ten ticks after the A of
#     the stow walk he is at boat local x -9.13 and the fish is at **-9.03**;
#     the old fixed value is -7.40, so this row is the whole difference.
#     **And it is in his own frame now as well as at his own position**, which
#     is the second half of the same defect. The hang was 1.55 m toward her
#     *port beam* whichever way he was looking, because `fx`/`fz` is a world
#     heading and the rotation into hers was solved one script away. Turning to
#     walk to the fish box swung the payoff out over open water -- which is
#     exactly the "floating off the side of the ship" the player reported.
#     `state.bfx`/`bfz` is that rotation, published this round for the rod, and
#     the fish is the second thing it buys. Ten ticks after the A of the stow
#     walk he is at boat local x -9.13 and the fish is at **-9.47**.
#     **Measured on the rod-tip pose now**, because the conger this tape lands
#     will not go in the creel and stays on the line: `Catch` at boat local x
#     -8.839 with the player at -9.126, ten ticks after the A of the stow walk.
#     The old fixed value was -7.40, so the row is the same difference it always
#     was; it is the *pose* that changed, not the claim.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1960 \
  --hold "$DEMO_FIGHT; 1900:move_x=-1; 1950:move_z=-1" \
  --assert "state.online == 1" --assert "Rig/Boat/Catch.local_x < -8.5" \
  --assert "state.plx < -8.9" >/dev/null

# 5g. **PHASE 4 — 95% OF A FISHING ROUND, AND IT HAD NO ROW.** 5f above asserts
#     phases 0, 1 and 3, which are exactly the three states the round that wrote
#     it had rendered and looked at. The fight runs tick 526 to 1850 — 1324 of
#     the 1390 ticks of a round, twenty-two seconds — and it was the state
#     nobody checked and the state that was broken: `float_bob.rhai` computed
#     `y -= load * 0.30`, so the harder the fish pulled the deeper the only
#     object in the frame went. At stress 59, 72 and 82 the float was **not in
#     the frame at all**. A gate assembled from the states you already inspected
#     cannot find the state you did not.
#
#     Two assertions, and they are deliberately different shapes.
#
#     `bob` is peak-to-peak vertical travel over the last 300 ticks, so it is
#     **tick-independent** — it says the float is working, at whatever instant
#     you stop the run. Measured across the fight it tracks the load: 0.32 at
#     stress 37, 0.48 at 60, 0.55 at 82. The old code gives 0.11-0.25 at every
#     load, because its only oscillation was the idle bob.
#
#     `local_y > 0.0` is one pinned tick and says the float **comes back up**:
#     the ball's centre is above the waterline, so 0.26 m of orange is out of
#     the water while the fish is pulling at stress 72. Under the old code that
#     tick reads about -0.29 and the ball is entirely submerged. `bob` alone
#     cannot see this — a float bucking two metres under the surface scores the
#     same — and `local_y` alone cannot see the mean, so it takes both.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1770 --hold "$DEMO_FIGHT" \
  --assert "state.phase == 4" --assert "state.stress > 60" \
  --assert "Rig/Boat/Float.bob > 0.45" \
  --assert "Rig/Boat/Float.local_y > 0.0" >/dev/null

# 5h. **THE HALF OF THE LOOP WITH NO KEYBOARD PROOF, AND IT IS THE HALF THAT
#     WAS BROKEN.** 5d ends at the landing. Everything after it — stow, home,
#     ashore, crate — was proved only by `rig_trip.loom`'s `Pilot` script, and
#     the first thing found in the untested half was that the demo's climax
#     pointed the player the wrong way: `the fish box is to starboard` is true
#     of the *boat* and he is standing in his own frame with his back to it.
#
#     Same tape as 5d, then two keys: **A** to clear the bait box aft, **S** to
#     cross to the box. That is the route the caption's own bearing describes —
#     `3 m dead behind you` — plus the one dogleg the deck furniture forces.
#
#     **And then E, which is the round's other half.** Walking up to the hatch
#     used to be the whole of it: the catch went below because the player had
#     been near a box, which is the one beat in the loop where the game played
#     itself. `DEMO_BOX` presses at 2000, standing 0.84 m off it. `carried`
#     falls 4 to 3, `infish` to 0, `stowed` to 1.
#
#     **Falsifiable by deletion**: with the E at 2000 taken out he stands at the
#     hatch for the rest of the run and every one of these four assertions
#     fails.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2300 --hold "$DEMO_BOX" \
  --assert "events.stow >= 1" --assert "state.stowed == 1" \
  --assert "state.infish == 0" --assert "state.carried == 3" >/dev/null

#     **AND THE LEG AFTER IT, WHICH WAS THE ONLY ONE WITH NO BEARING AT ALL.**
#     `toward` was called for the fish box and for the crate once he was already
#     ashore; the leg between them read `N BELOW   take her alongside and step
#     ashore` — no direction, no distance — and `at_helm` outranks `aboard`, so
#     for the whole of the drive home the line said nothing about home while the
#     one key the demo teaches drove him further out. Same tape as 5h, then the
#     reverse dogleg back to the wheel: forward up the port side, starboard onto
#     the mat, then ahead. Both sentences are asserted.
#
#     **The second one used to read `CRATE 12 m` and the comment here used to
#     say "the number is what tells him W was the wrong key — it counts up".**
#     That is a gate row pinned on the player doing the wrong thing, and it is
#     the wrong instrument: the whole return leg is driven stern-first at a
#     wharf the helmsman cannot see, and asking him to notice a rising number is
#     asking him to run the experiment. The line names the key now. It is
#     `HOME 19 m` rather than `CRATE 12 m` because the range is measured from
#     the **hull** — the same distance `ALONGSIDE` judges the delivery by —
#     rather than from a man standing seven metres forward of her origin, and
#     because the key it names is a fact about which way *she* points.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2120 --hold "$DEMO_BOX" \
  --assert "state.stowed == 1" --assert "state.aboard == 1" \
  | grep -q '"message": "1 BELOW   the crate is 5 m behind you, on your left'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2200 --hold "$DEMO_HELM" \
  --assert "state.stowed == 1" --assert "state.at_helm == 1" \
  | grep -q '"message": "THE HELM   AHEAD   wheel amidships   5 kn   SPACE lets go   HOME 19 m — hold S"'

# 5i. **AND THE TWO SENTENCES THAT GET HIM THERE, ASSERTED AS SENTENCES.**
#     `--assert` has no `message` axis, so the same `grep` the two fight endings
#     use is what can read these. Both are round-8 regressions waiting to
#     happen, and neither has a numeric shadow.
#
#     **One: the bearing is in the player's frame and it persists.** The landing
#     notice used to expire after two seconds and hand the line back to `ABOARD
#     CLICK to cast` while `state.infish` still read 1 and the fish was hanging
#     in front of him. This reads the caption two hundred ticks *after* the
#     notice window has closed, so it fails if either the persistence or the
#     frame regresses.
#
#     **Round 9: what it says at the wheel is the ROUTE, not the bearing, and
#     that is the fix rather than a wording change.** The bearing was correct
#     and the walk it implied was not: `col_engine_box` stands 0.50 m proud
#     between the wheel and the box, ten tapes from the landing say S alone, A
#     alone and D alone all wedge for five hundred ticks, and the sentence went
#     on saying `3 m dead behind you` while he was pressed into it. The route
#     line is drawn only while he is forward of the box and inside its x band —
#     so this row and the next are one measurement in two halves.
#     **`$DEMO_CONGER` and tick 2250, not the plain tape and 2100.** The
#     conger this demo lands will not go in the creel on the first press, so on
#     the plain tape at 2100 she is still on the line and the caption is the
#     rod's. `$DEMO_CONGER` is the tape that empties a cell for her; 2250 is
#     past its notice window, which is the whole point of the row — it reads the
#     *persistent* branch and fails if the bearing stops being drawn.
#     **`FISH IN HAND` is `FISH IN THE CREEL` now**, because that is where she
#     is: the hand is a thing that exists only while the grid is open.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2250 --hold "$DEMO_CONGER" \
  --assert "state.fishheld == 1" --assert "state.stuck == 0" \
  | grep -q '"message": "FISH IN THE CREEL   go LEFT round the bait box'

#     **And the other half: once he is round it, the steerable bearing is back.**
#     Ten ticks after the A the route hint is gone and the metres are counting
#     down. Without this row the route line could be latched on for ever and
#     nothing would notice; with it, both branches are pinned.
#     **`$DEMO_CONGER` again, and the walk starts after the creel closes.**
#     Once she is in the creel and he is round the bait box the bearing counts
#     down: `1 m ahead on your right` at 2260, with `stuck` at 0, so the route
#     hint really did clear rather than being latched on for ever.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2260 \
  --hold "$DEMO_CONGER; 2110:move_x=-1; 2200:move_z=-1" \
  --assert "state.fishheld == 1" --assert "state.stuck == 0" \
  | grep -q '"message": "FISH IN THE CREEL   E at the YELLOW FISH BOX — 1 m ahead on your right"'

#     **And the sentence a player meets when she will not go in.** Notice 4,
#     with her size in it: "full" was the only thing four slots could ever say,
#     and a creel can say *she needs 2x3 and you have not got 2x3 anywhere*,
#     which is a problem with a solution rather than a dead end.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1960 \
  --hold "$DEMO_FIGHT; 1900:move_x=-1,interact=1; 1901:move_x=-1; 1950:move_z=-1" \
  --assert "state.online == 1" --assert "state.infish == 0" \
  | grep -q '"message": "SHE WILL NOT FIT   that conger is 2x3 — TAB to make room, or E at the YELLOW FISH BOX"'

#     **Two: a player wedged aboard is told he is wedged.** `deeper_player.rhai`
#     could not fire `stuck` aboard at all — the flag existed and the one place
#     in this demo where being blocked is confusing was the one place it was
#     gated out. Holding S alone from the landing walks him into
#     `col_engine_box` and he stops dead: z goes -12.304 to -11.601 and never
#     moves again. `state.stuck` is the numeric half and the sentence is the
#     half a player reads.
#
#     **And it names the thing and the hand now.** `something is in the way — go
#     round it` told a wedged player neither which object nor which of the two
#     directions clears it, and those two are the only inputs he is not already
#     pressing. This row is the reason the wedge is worth keeping: it is the
#     recovery, and a recovery with no instruction is a lost demo.
#
#     **`$DEMO_CONGER` and not the plain tape, and the reason is priority.** A
#     fish still on the line outranks the wedge in the caption chain, and on the
#     plain tape she *is* still on the line — the conger is refused at the rod.
#     `$DEMO_CONGER` puts her in the creel first, so what this row measures is
#     the wedge and not the rod.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2400 \
  --hold "$DEMO_CONGER; 2150:move_z=-1" \
  --assert "state.stuck == 1" --assert "state.aboard == 1" \
  --assert "state.infish == 1" \
  | grep -q '"message": "BLOCKED   the bait box — step LEFT and go round it"'

#     **And the control that keeps that honest**: the one gesture this demo
#     teaches must *not* trip it. Boarding, the capsule stands still for eighty
#     ticks while it climbs the treads with the deck heaving under it, and at
#     the ashore threshold the HUD printed `BLOCKED  go round it` at a player
#     whose correct instruction is *keep holding W*. Without this row the
#     aboard threshold can be tuned down to nothing and every row above still
#     passes.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 320 --hold move_z=1 \
  --assert "state.stuck == 0" --assert "state.aboard == 1" >/dev/null

# 5j. **THE FOUR SLOTS FILL IN 1.3 SECONDS AND USED TO DO IT IN SILENCE.**
#     `state.carried` goes 0 to 4 between ticks 20 and 80 with the caption
#     reading `WALK FORWARD TO THE BOAT` throughout — one of the four things
#     this demo *is*, arriving before the player has understood he has an
#     inventory. The numbers row changed; nothing told him to look at it.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 40 --hold move_z=1 \
  --assert "state.carried == 2" \
  | grep -q '"message": "PICKED UP BAIT'

# 5l. **THE CREEL ITSELF, AS A PICTURE, WHICH NOTHING ELSE CAN SEE.**
#
#     **`state.slots` is gone and these three greps are its replacement**, named
#     here because deleting a pinned string silently is how a gate stops
#     checking a thing. It was four five-wide cells — `[FLASK][BAIT ][     ][
#     ]` — and it could say what was in each slot and nothing about *where*.
#     `state.creel_cells` is one letter per placement per cell, rows joined by
#     `/`: `abc/d../...` is four one-cell items in the first-fit packer's only
#     right answer, and `a.b/c../...` has a hole in it where the bait was.
#
#     **A string the game invents is not assertable and `--assert` is not
#     getting a string axis** — see `GameState::text`. So `loom sim` prints the
#     strings a rules script keeps beside the numbers, and these rows `grep`
#     them, which is exactly how every caption in this file is already pinned.
#
#     Two cells on the way to the boat, four at the wheel, and — after the
#     conger the demo's own tape lands — three, with the hole the spent bait
#     left and a fish in his hands that will not go in it.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 40 --hold move_z=1 \
  | grep -q '"creel_cells": "ab\./\.\.\./\.\.\."'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_z=1 \
  --assert "state.creel_free == 5" \
  | grep -q '"creel_kinds": "FLSK BAIT LINE LAMP"'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1910 \
  --hold "$DEMO_FIGHT; 1900:interact=1; 1901:" \
  --assert "state.online == 1" \
  | grep -q '"creel_cells": "a\.b/c\.\./\.\.\."'
#     And the hand's own label, from the one place a fish is ever in it: lifted
#     back out of a cell with the grid open.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2180 \
  --hold "$DEMO_CONGER; 2120:bag=1; 2121:; 2140:move_x=1; 2148:; \
2170:interact=1; 2171:" \
  | grep -q '"creel_hand_label": "><>"'

# 5k. **THE HELM SAYS WHAT IT IS SET TO.** The old caption listed four bindings
#     and a speed and never once stated the state of either control, so a player
#     holding A had no confirmation the wheel was over except channel marks
#     going past. "Show it in the world" cannot answer this one: the boat's
#     drawn wheel and throttle are at boat-local (2.38, 4.24) and (2.45, 4.18),
#     up on the flybridge, ten metres forward of the helm mat and three and a
#     half above it — measured off the OBJ vertex bounds — so the helmsman
#     cannot see either lever. Three rows: hands off it teaches the keys, ahead
#     with the wheel over it names both settings, and astern is the other sign.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 500 --hold "0:move_z=1; 400:" \
  --assert "state.at_helm == 1" \
  | grep -q '"message": "THE HELM   W ahead  S astern'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 \
  --hold "0:move_z=1; 400:move_z=1,move_x=1" --assert "state.at_helm == 1" \
  | grep -q '"message": "THE HELM   AHEAD   wheel to starboard'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 600 \
  --hold "0:move_z=1; 400:move_z=-1,move_x=-1" --assert "state.at_helm == 1" \
  | grep -q '"message": "THE HELM   ASTERN   wheel to port'

# 6. **"Am I moving?" as a number, because the picture will not say.** From the
#    helm at the opening yaw a frame after forty-five metres of travel differs
#    from a moored frame by less than two frames of the same moving boat eleven
#    seconds apart: the mat faces her beam and nothing fixed is ever in shot.
#    The picture fix is to berth her bow-out and it moves every pinned number
#    above. `state.knots` is the readout in the meantime — and the pair is what
#    makes it a claim rather than a display, because a smoother that had jammed
#    at a constant would pass either row alone.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_z=1 \
  --assert "state.knots > 5.0" >/dev/null
"$LOOM" sim assets/test/rig_drive.loom --ticks 900 \
  --assert "state.knots < 0.5" >/dev/null

# 7. **THE ONE THING ABOUT THE SOUND THAT CAN BE ASSERTED.** `loom audio` mixes
#    the weather bed and does not touch `AudioSource` — it reports `rms 0.0` on
#    `proving_ground.loom`, which has one — so there is no headless path to a
#    mixed frame and no gate can hear this demo. What *is* checkable is the
#    mechanism: `sound.rs` starts a voice only for an `autoplay` source and a
#    node script may write only a transform, so the engine note is a source that
#    **moves**, and `--assert` reads transforms. Moored she is 12 m under the
#    keel against a 14 m range, which is silence; at 5.8 kn she is under the
#    cockpit sole. If `engine_note.rhai` or `state.knots` breaks, these two
#    numbers collapse together and this is the only thing that would notice.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 300 \
  --assert "Rig/Boat/Engine.local_y < -11.0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_z=1 \
  --assert "Rig/Boat/Engine.local_y > -1.0" --assert "state.knots > 5.0" >/dev/null

# **THE LOOP, END TO END, IN ONE PROCESS.** Five rounds built four features and
# nothing had ever crossed from one to the next: no run in this file had taken
# the boat out and brought it back, so "you can leave" and "you can return" were
# two claims about a scene rather than one claim about a trip.
#
# `rig_trip.loom` is the demo plus a `Pilot` node. Everything else is the
# shipped scene, and the player is driven by a **scheduled** `--hold` — the same
# `Runner::input` a human writes, now taking `<tick>:<keys>` segments. See that
# file's header for why a tape is right here and a movement pilot is not, and
# `crates/loom_cli/src/main.rs` for why the one-constant form stopped being
# enough (a loop is the first question two separate runs cannot answer: the
# fish, the boat's position and the hold have to survive the gap).
#
# The tape, in the order a player would press it:
#
#   0     W          walk aboard, four supplies, on the mat by 356, ahead
#   900   S          astern for home; the fish lands at 1147 on the way
#   1200  E          she comes off the line and into his hands
#   1250  SPACE      hands off the wheel
#   1260  S + D      across the cockpit toward the fish box
#   1320  W + A      back to the mat, which takes the helm again
#   1380  S          astern the rest of the way; alongside by 2000
#   2000  -          throttle shut, she settles
#   2060  SPACE      hands off
#   2070  S          over the boarding steps -- and the walk passes the fish box
#   2100  E          STOWED, 1.23 m off it
#   2150  A + S      west along the wharf to the crate
#   2200  E          DELIVERED
#
# **The three E presses are this round and the tick they are at is measured, not
# guessed.** The comment above used to say "STOWED at 1310", and that was wrong
# in a way only a press could expose: the closest this walk ever brings him to
# the hatch between 1260 and 1400 is **2.08 m**, against a `RANGE` of 2.0. The
# proximity stow it described actually fired on the way *ashore*, somewhere
# between 1900 and 2100. A press has to be aimed, so it found out.
#
# **Four assertions, and each is a different link in the chain.** `stow` says
# the catch went below on a walk; `deliver` says the trip closed; `stowed == 0`
# says the hold was emptied by it and not merely counted twice; `delivered == 1`
# is the number the HUD prints and the only thing in this demo that persists
# across a trip.
"$LOOM" sim assets/test/rig_trip.loom --ticks 2210 \
  --hold "0:move_z=1; 900:move_z=-1; 1200:move_z=-1,interact=1; 1201:move_z=-1; \
1250:jump=1; 1260:move_z=-1,move_x=1; \
1320:move_z=1,move_x=-1; 1380:move_z=-1; 2000:; 2060:jump=1; 2070:move_z=-1; \
2100:move_z=-1,interact=1; 2101:move_z=-1; \
2150:move_x=-1,move_z=-0.15; 2200:move_x=-1,move_z=-0.15,interact=1; \
2201:move_x=-1,move_z=-0.15" \
  --assert "events.landed >= 1" --assert "events.take >= 1" \
  --assert "events.stow >= 1" \
  --assert "events.deliver >= 1" --assert "state.delivered == 1" \
  --assert "state.stowed == 0" >/dev/null

# **The control, and without it the "bring her back" half is dead code.**
# `deeper_rules.rhai` refuses the crate unless `Rig/Boat` is within `ALONGSIDE`
# — 14.0 m — of it. `rig_trip` brings her home, so deleting that check leaves
# every row above passing unchanged. `rig_adrift.loom` leaves her at (3.0, -24),
# 21.0 m off, and swims the player home instead: he lands a fish, stows it,
# steps off her into the sea, swims fourteen metres to `Rig/Ladder`, walks up
# it, reaches the crate — and is refused, with `state.stowed` still 1.
#
# **It is also the ladder's own row.** The swim is a real distance to the new
# ramp on the berth side, and `Rig/Player.y > 2.2` at the end is a capsule
# standing on the rig deck under its own movement, with no teleport.
#
# **The tape was re-pinned when the deckhouse opened**, deliberately, and both
# changes make it a better tape than it was. `1460` used to press **forward**
# and starboard at a fish box that is *aft* and starboard: it reached the box
# by being stopped by the deckhouse's after face, and the moment that face
# grew a door the same press walked him into the salon and left him there. It
# now presses at the box. `1650`'s westing came down from 1.0 to 0.4 because
# the boat's new underbody boxes hold a swimmer 0.35 m off her side instead of
# letting him leave through her, and the old number then overshot `Rig/Ladder`
# — which is 1.4 m wide between its kerbs — by 1.26 m to the west. Measured:
# he lands at (-9.51, 2.30, -0.38), well up the deck.
#
# **And the refusal is now pressed for, which it never was.** The crate was a
# place: he walked past it and was told no. `deliver == 0` on a run where nobody
# ever asked is an assertion that cannot fail, so the tape now presses E at 1920
# -- 1.86 m off the crate, measured, inside `RANGE` -- and the refusal sentence
# is greped. E at 1440 takes the catch off the line and E at 1540 stows it, both
# at measured distances.
"$LOOM" sim assets/test/rig_adrift.loom --ticks 2000 \
  --hold "0:; 1440:interact=1; 1441:; 1450:jump=1; 1460:move_z=-1,move_x=-1; \
1520:move_z=-1; 1540:move_z=-1,interact=1; 1541:move_z=-1; \
1650:move_z=-1,move_x=-0.4; 1880:move_z=-1; \
1920:move_z=-1,interact=1; 1921:move_z=-1" \
  --assert "events.stow >= 1" --assert "state.stowed == 1" \
  --assert "events.deliver == 0" --assert "state.delivered == 0" \
  --assert "Rig/Player.y > 2.2" \
  | grep -q '"message": "THE CATCH IS IN HER HOLD   bring her alongside first"'

# **THE DECK STILL CARRIES A MAN WHEN THE WEATHER GETS UP, AND NOTHING ELSE
# HERE CAN SEE THAT.** ADR 0060 states an acceptance — 50 mm net drift and
# 30 mm peak-to-peak, boat-frame, per rider — and until this row it was checked
# by hand, once, at one wind speed. No pixel diff can see a rider ratchet up a
# metre and freeze there; no determinism hash moves when he does, because he is
# a character driven by a script and not a rigid body; and `events.station` is
# an integral that cannot see a man bouncing ninety centimetres off a deck.
#
# `weather_ramp.loom` is `jib_vi_drift.loom` — the five-station carry
# instrument — with a wind ramp over it, so the reading is taken at the top of
# a ladder instead of in a millpond. `dread` saturates at tick 1800.
#
# **Four ticks over one wave period, and that is why this is four runs.** The
# acceptance has two halves and a single tick count can only see the first: a
# rider that swings 25 mm and comes back reads as zero net drift. The top
# rung's peak wavelength is about 7 m, so its period is 2.1 s and 1800/1830/
# 1860/1890 are its quarters.
#
# **`local_z` is the axis that goes first** — the sea runs 20° off her bow, so
# the beam is where the deck moves. The band is 22 mm wide about each station:
# inside ADR 0060's 30 mm, and wider than the 15.8 mm this configuration
# actually reaches, so it is a regression bound rather than a restatement of
# today's number.
#
# **Fault-injected, four ways, because a carry row that cannot fail is worse
# than none.** Raising the ladder's top rung to 18 fails it, and to 20 fails it;
# lowering it to 13 passes; deleting the ladder's `wind_speed` passes *this*
# pair and fails the wind pair below — which is why both exist. The wind cap of
# 16 in that scene is this measurement and not a preference.
"$LOOM" sim assets/test/weather_ramp.loom --ticks 1800 \
  --assert "Sea/Boat/PortRail.local_z < -2.290" \
  --assert "Sea/Boat/PortRail.local_z > -2.312" \
  --assert "Sea/Boat/BridgeDeck.local_z < -2.090" \
  --assert "Sea/Boat/BridgeDeck.local_z > -2.112" >/dev/null
"$LOOM" sim assets/test/weather_ramp.loom --ticks 1830 \
  --assert "Sea/Boat/PortRail.local_z < -2.290" \
  --assert "Sea/Boat/PortRail.local_z > -2.312" \
  --assert "Sea/Boat/BridgeDeck.local_z < -2.090" \
  --assert "Sea/Boat/BridgeDeck.local_z > -2.112" >/dev/null
"$LOOM" sim assets/test/weather_ramp.loom --ticks 1860 \
  --assert "Sea/Boat/PortRail.local_z < -2.290" \
  --assert "Sea/Boat/PortRail.local_z > -2.312" \
  --assert "Sea/Boat/BridgeDeck.local_z < -2.090" \
  --assert "Sea/Boat/BridgeDeck.local_z > -2.112" >/dev/null
"$LOOM" sim assets/test/weather_ramp.loom --ticks 1890 \
  --assert "Sea/Boat/PortRail.local_z < -2.290" \
  --assert "Sea/Boat/PortRail.local_z > -2.312" \
  --assert "Sea/Boat/BridgeDeck.local_z < -2.090" \
  --assert "Sea/Boat/BridgeDeck.local_z > -2.112" >/dev/null

# **And the wind on that ladder actually rises, which is the other half.** A
# ramp wired to nothing would pass every row above it: the riders would sit
# still because the sea never got up, and the carry rows would report a full
# pass having never looked at a rough sea. Deleting `wind_speed` from the scene
# fails this pair and nothing else in the five gates.
#
# `Wind.speed` runs 3.5 -> 16 over the ramp. These are the *measured* readings
# rather than the authored ones — the authored value is a free-stream number
# about 10% above U10 and the probe is at 3 m.
"$LOOM" sim assets/test/weather_ramp.loom --ticks 60 \
  --assert "wind@0,3,0.speed < 4.0" >/dev/null
"$LOOM" sim assets/test/weather_ramp.loom --ticks 1800 \
  --assert "wind@0,3,0.speed > 9.0" >/dev/null

# **THE HULL IS A SOLID OBJECT, WHICH IT WAS NOT.** Three faults, one cause:
# the hull's own `BoxCollider` is demoted to mass-only the moment the deck
# prefab attaches a plate (ADR 0060), and every box that prefab had started at
# or above y = 0.30 — the waterline is 0. So the boat was drawn solid and
# collided as a lid with a deckhouse on it.
#
# **You cannot swim through her.** Spawned two metres off her port topside and
# swimming at her for thirty seconds. Before the underbody boxes this run ended
# at z = +2.87 — through the hull, out the far side, thirteen metres past her,
# at a flat 1.9 m/s with no deceleration anywhere. `-13.2` is her drawn topside
# *at the waterline*, which is what a swimmer meets; the 3.30 m half-beam in her
# header is the deck edge a metre and a half higher up.
"$LOOM" sim assets/test/rig_underhull.loom --ticks 600 --hold move_z=-1 \
  --assert "Rig/Player.z < -13.2" >/dev/null

# **And you can get inside the deckhouse, which is the other end of the same
# bug.** The refined hull has a salon with a 0.92 m door in its after bulkhead
# and 2.2 m of headroom; `col_house` was one solid box over the whole footprint,
# authored on a headroom measurement taken off the *previous* hull mesh. Walking
# forward from the cockpit on the door's own centreline, this run used to end at
# boat-local x = -6.57 — pressed on the outside of the bulkhead for five
# seconds. It now reaches +1.83, at the salon's forward end.
"$LOOM" sim assets/test/rig_salon.loom --ticks 300 --hold move_x=1 \
  --assert "state.plx > -4.0" >/dev/null

# **And falling off her is survivable a hundred metres out.** `rig_overboard`
# is this claim about the rig; there was no equivalent for the hull, and at the
# fishing ground the two rig ramps are 88 m and 114 m away — a two-minute swim
# away from a boat that is then adrift with the catch in it.
# `Rig/Boat/SternLadder` is a 45 degree slab off her port quarter reaching
# y = -1.80, which is under a floating swimmer's feet.
#
# Row 1 is the control that makes row 3 mean anything: without it a run that
# ended on the deck could have ended there because the swimmer never got wet.
# Row 2 is the caption, and it is the half that only exists because the ladder
# moves — a sentence naming a rig you cannot see is the lie round 5 told, one
# target further on.
"$LOOM" sim assets/test/rig_reboard.loom --ticks 240 \
  --assert "Rig/Player.y > -1.0" --assert "Rig/Player.y < 0.0" >/dev/null
"$LOOM" sim assets/test/rig_reboard.loom --ticks 60 \
  | grep -q '"message": "IN THE WATER   swim east to her stern ladder"'
"$LOOM" sim assets/test/rig_reboard.loom --ticks 900 --hold move_x=1 \
  --assert "Rig/Player.y > 0.9" --assert "state.aboard == 1" >/dev/null

# 7. **The stall limiter says so now.** It shut the throttle in round 3 and the
#    caption went on reading "W ahead", so a player pushing a bow into a quay
#    got a boat that had silently stopped obeying him. Same run as the
#    `rig_bump` row above; this is the half of it the player can see.
"$LOOM" sim assets/test/rig_bump.loom --ticks 2400 --hold move_z=1 \
  --assert "state.jammed == 1" >/dev/null

# ---------------------------------------------------------------------------
# 8. **THE EDGES — inputs nobody would write a tape for.**
#
# Every row above this line is a tape somebody wrote *after* watching the thing
# work, which is the shape of test this block exists to break. Fourteen scenes
# asserted and not one wrong keypress among them: a gate assembled out of the
# author's own successful runs cannot find the state the author never entered.
# Eleven lines of deliberately stupid input found three defects in one pass, and
# all three are fixed above — the buoy stall, the ashore `BLOCKED` sentence that
# told a man walking backwards to back up, and a helm caption with no way home
# on it.
#
# **These are cheap and they are the only rows here that are adversarial.** Add
# to them before adding another pinned tick to a tape that already passes.

# 8a. **The whole trip, on the shipped file, from the keyboard.** Every
#     `delivered` assertion in this gate used to be on `rig_trip.loom`, so the
#     demo's headline claim — you can complete a trip — was asserted about a
#     scene one node away from the shipped one rather than about the shipped one
#     itself. This is the 5i tape carried on: astern at 2200, throttle shut at
#     2600, SPACE at 2660, over the boarding steps, west along the wharf.
#     3,800 ticks, 4.6 s.
#
#     **`rig_trip.loom` is not a fork and the note that called it one was
#     wrong.** It is 103 lines of which 16 are not comment: an `extends` of
#     `deeper_demo.loom` plus a `Pilot` node. It cannot drift from the demo,
#     because it *is* the demo. That correction removes the case for retiring
#     it — what remains is that a claim should be asserted about the file people
#     run, which is what this row does, and `rig_trip` keeps its own row below
#     because a pilot-driven fight and a keyboard-driven one are two paths.
#
#     **And it is where the two arrival sentences are asserted**, because they
#     only exist on a run that actually arrives. `ALONGSIDE — press SPACE`
#     replaces `HOME nn m — hold S` at 2500 (she is 2.73, -10.48; the crate is
#     inside `ALONGSIDE`'s 14.0 m), and the aboard line says `step off` instead
#     of `take her alongside`. Traced before the fix: at 6 m and again at 2 m
#     the demo was still telling the player to bring the boat in, at the exact
#     beat it pays off, and no branch anywhere said to get out of her.
#
#     The tape itself is `DEMO_HOME`, declared with its three siblings above 5d.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2500 --hold "$DEMO_HOME" \
  --assert "state.stowed == 1" --assert "state.at_helm == 1" \
  | grep -q '"message": "THE HELM   ASTERN   wheel amidships   3 kn   SPACE lets go   ALONGSIDE — press SPACE"'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2800 --hold "$DEMO_HOME" \
  --assert "state.stowed == 1" --assert "state.at_helm == 0" \
  | grep -q '"message": "1 BELOW   SHE IS ALONGSIDE — step off, E at the crate 5 m dead behind you"'
#     **`events.take` has left this row and `events.refused` has joined it.**
#     The conger this tape lands will not go in the creel, so the first E is
#     refused and she stays on the line — and the second E, at the fish box,
#     takes her off it and puts her below in the one press. That is the escape
#     hatch working, and it is why the hatch reads `online` as well as the
#     creel. `take` is covered where it can happen, in 5b4.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 3800 --hold "$DEMO_HOME" \
  --assert "events.landed >= 1" --assert "events.refused >= 1" \
  --assert "events.stow >= 1" --assert "events.deliver >= 1" \
  --assert "events.use == 3" \
  --assert "state.delivered == 1" --assert "state.stowed == 0" \
  | grep -q '"message": "IN THE CRATE   that is a trip. Take bait and go again"'

# 8b. **Press nothing for twenty seconds.** The one thing a stranger who has
#     read nothing will do first. He keeps the supply he spawned on top of and
#     the instruction does not decay into anything else.
#
#     **The tail is new and is the mirror's only signpost.** The glass is 1.7 m
#     due south of the spawn and he starts facing away from it, because the rig
#     is turned so the berth is on -Z and neither an authored eye yaw nor an
#     authored character yaw can say "start facing this way". Nothing else in
#     the demo tells him it is there. It is gated on 3.5 m from the glass, so
#     this row is also what would catch it following him to the berth.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1200 --hold "0:" \
  --assert "Rig/Player.y > 2.2" --assert "state.carried == 1" \
  | grep -q '"message": "WALK FORWARD TO THE BOAT   — turn around: there is a GLASS on the shed"'

# 8c. **Press everything at once.** W, D, SPACE, SHIFT and the trigger, held for
#     thirty seconds. He sprints diagonally off the rig into the sea — and the
#     claim is not that he stays dry, it is that the sea is the safety device:
#     he floats rather than falling forever (`y > -1.0`, against -202 for the
#     capsule that fell through in round 1) and the caption computes a compass
#     word from where he is. `rig_overboard.loom` above is the row that proves
#     a swimmer walks back out; this is the row that proves the masher gets
#     there.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1800 \
  --hold "0:move_z=1,move_x=1,jump=1,sprint=1,fire=1" \
  --assert "Rig/Player.y > -1.0" --assert "state.swimming == 1" \
  | grep -q '"message": "IN THE WATER   swim south-west to the ramp"'

# 8d. **Hard over, both ways, and they are not the same.** Board on W, then hold
#     the wheel hard across. To starboard is open sea and she keeps making way.
#     To **port** — the side the demo teaches a player to watch, because the
#     channel marks stream past there — she runs into `Mark5` at (44, -20) and
#     stops: 0.89 knots at tick 1250, 0.58 at 2000, still at full ahead.
#
#     **The stall limiter could not see it and this is why the second detector
#     exists.** `deeper_player.rhai` measures the *helmsman's* speed, and a hull
#     pinned on a buoy pivots on it — he stands seven metres off her centre and
#     keeps moving. `state.pinned` is the rules script's, off her own.
#
#     **The port row reads at 1800 and not 2000, and the reason is issue 6 in
#     the scene's own list.** `pinned` resets whenever `at_helm` does, and a
#     hull grinding on a buoy bounces the helmsman off the mat for a few ticks
#     at a time. Sampled on the shipped scene: latched at 91 from about 1350 to
#     1950, then 2 at 2000, 0 at 2050, 1 at 2100. Tick 2000 sat one flicker
#     from the edge and the boat's new underbody boxes moved her 12 cm at that
#     tick, which was enough. 1800 is the middle of the window and reads 91 on
#     both sides of that change.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2000 \
  --hold "0:move_z=1; 500:move_z=1,move_x=1" \
  --assert "state.pinned == 0" --assert "state.knots > 4.0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1800 \
  --hold "0:move_z=1; 500:move_z=1,move_x=-1" \
  --assert "state.pinned > 90" --assert "state.knots < 1.0" \
  | grep -q '"message": "PUSHING ON SOMETHING   S to back off"'

# 8e. **And the advice works**, which is the half a caption gate cannot claim.
#     Same tape, S from 1500: she backs off the mark and is 27 m clear of it by
#     2600, with the latch released.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2600 \
  --hold "0:move_z=1; 500:move_z=1,move_x=-1; 1500:move_z=-1" \
  --assert "state.pinned == 0" --assert "Rig/Boat.z > 0.0" >/dev/null

# 8f. **Hold W until the rig is out of sight.** The one gesture this demo
#     teaches, at the wheel, is out to sea: 6,000 ticks of it reaches x = 282
#     with nothing in any direction but water, and the range home was gated on
#     `stowed > 0` — so the player who had not yet caught anything, which is
#     every first-timer, got no number at all. The bearing is the hull's, not
#     the helmsman's: at the wheel he faces her beam, so "behind you" and
#     "astern" are ninety degrees apart and only one of them is a throttle
#     setting.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1600 --hold move_z=1 \
  | grep -q '"message": "THE HELM   AHEAD   wheel amidships   5 kn   SPACE lets go   HOME 72 m — hold S"'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2600 --hold move_z=1 \
  --assert "Rig/Boat.x > 110.0" \
  | grep -q '"message": "THE HELM   AHEAD   wheel amidships   5 kn   SPACE lets go   HOME 122 m — hold S"'

# 8g. **Walk into a wall ashore, two ways.** Hold S from the spawn and back into
#     the north rail; hold D and strafe into the shed. Both used to print
#     `BLOCKED   back up — aboard is the gap between the bollards` — a fixed
#     string that tells a man already walking backwards to back up, and names a
#     gap that may be behind him. The aboard arm of the same sentence has been
#     computed since round 9; that asymmetry was the bug.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_z=-1 \
  --assert "state.stuck == 1" \
  | grep -q '"message": "BLOCKED   something has you — she is 10 m straight ahead"'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_x=1 \
  --assert "state.stuck == 1" \
  | grep -q '"message": "BLOCKED   something has you — she is 19 m ahead on your left"'

# **The interact channel (E), which is the sixth digital one.** The engine does
# nothing with `interact` by itself — a script is the only thing that can — so
# the only observable end of it is a script saying it saw a press.
# `interact_probe.rhai` emits `used` per tick the channel is true and
# `used_edge` per rising edge, and the pair is what separates the two drivers.
#
# One scheduled press: one tick, one edge, and this is the row that fails if
# the channel is dropped anywhere between `--hold` and the rhai scope.
"$LOOM" sim assets/test/interact_probe.loom --ticks 120 --hold "30:interact=1; 31:" \
  --assert "events.used == 1" --assert "events.used_edge == 1" >/dev/null

# Held to the end of the run: 91 ticks, still one edge. **`--hold` is level by
# design** — it writes `Runner::input` straight — so a consumer that must not
# repeat edge-detects for itself, exactly as `deeper_player.rhai` does for
# `fire` after `--hold fire=1` produced 708 casts. `loom run` cannot reach this
# state: `Play::set_input` takes the edge before the tick sees it, which is a
# Rust test (`play::tests::holding_interact_is_one_press`) and not a row here.
"$LOOM" sim assets/test/interact_probe.loom --ticks 120 --hold "30:interact=1" \
  --assert "events.used == 91" --assert "events.used_edge == 1" >/dev/null

# Two presses are two, which is what stops the row above from passing on a
# channel that latched true once and never cleared.
"$LOOM" sim assets/test/interact_probe.loom --ticks 120 \
  --hold "30:interact=1; 31:; 60:interact=1; 61:" \
  --assert "events.used == 2" --assert "events.used_edge == 2" >/dev/null

# **WHAT NO ROW ABOVE CAN SEE: the overlay.** Every caption above is pinned
# with `grep -q` on `loom sim`'s JSON, which proves the rules script *produced*
# a string. It cannot prove the string was legible, fitted, or was drawn at all
# — `Hud` draws only in `loom run`, and a headless render shows neither row.
# One row of this demo's HUD passed every gate in this project while being
# invisible on screen (cream glyphs on a near-white stone), and it was found by
# a human taking a screenshot.
#
# The one instrument that now exists is a unit test, not a row here:
# `loom_cli::hud::the_longest_demo_caption_fits_the_narrowest_documented_window`
# lays the two longest strings out through egui and measures them — 840 px for
# the caption, 621 px for the inventory row, against a documented 960 px minimum
# window. It runs under `cargo test`, which is check 3. **It measures width and
# nothing else**: contrast, occlusion and whether the line is drawn at all are
# still unmeasured, and the drop shadow that fixed the invisible row is asserted
# only as an offset.
#
# **The sim's answer to `xtask repeat`.** `state_hash` covers physics, so
# nothing above would notice a fight that replayed differently — a float hash,
# a map iterated in host order, a wall clock. Three fresh processes, compared
# byte for byte, is the same licence the GPU-stateful render paths hold.
"$LOOM" sim assets/test/fight_skilled.loom --ticks 1800 > /tmp/loom-fight-1.json
"$LOOM" sim assets/test/fight_skilled.loom --ticks 1800 > /tmp/loom-fight-2.json
"$LOOM" sim assets/test/fight_skilled.loom --ticks 1800 > /tmp/loom-fight-3.json
cmp /tmp/loom-fight-1.json /tmp/loom-fight-2.json
cmp /tmp/loom-fight-2.json /tmp/loom-fight-3.json
rm -f /tmp/loom-fight-1.json /tmp/loom-fight-2.json /tmp/loom-fight-3.json

# **And of the demo itself, which nothing asked.** `xtask repeat` derives its
# list from `GOLDEN` and `deeper_demo.loom` is in `SCENES` only, so the one
# scene in this repository with a whole game in it — a floating hull, a
# character riding it, a scripted fight, four node scripts writing their own
# transforms — had no byte-identity row anywhere. Round 8 is the round that
# found out this scene's physics hash is sensitive to things it should not be
# (see the `FishHold` block in the scene, and the round report), which is
# exactly when this becomes worth pinning: a hash that moves for a reason is
# fine, a hash that moves twice in one process is not.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1200 --hold move_z=1 > /tmp/loom-demo-1.json
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1200 --hold move_z=1 > /tmp/loom-demo-2.json
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1200 --hold move_z=1 > /tmp/loom-demo-3.json
cmp /tmp/loom-demo-1.json /tmp/loom-demo-2.json
cmp /tmp/loom-demo-2.json /tmp/loom-demo-3.json
rm -f /tmp/loom-demo-1.json /tmp/loom-demo-2.json /tmp/loom-demo-3.json

# **And the creel's interactive half, which the run above cannot reach.** That
# tape holds W and never presses TAB, so it covers the auto-place and the
# packer's self-check and none of the verbs. This one opens the grid, walks the
# cursor, lifts, places and ditches. It is the only check in this project that
# can see a creel that replayed differently: `state_hash` is physics-only and
# `cargo xtask repeat` derives its list from `GOLDEN`, which this scene is not
# in. `loom sim` prints every `state` name it keeps, so comparing the whole JSON
# byte for byte compares the whole creel.
#
# Traced, because a repeat tape that exercises nothing is three identical
# nothings: this one produces `cursor` 2, `lift` 2, `place` 1 and `ditched` 1.
# The first draft pressed E over an occupied cell and had no `place` in it.
CREEL_TAPE="0:move_z=1; 240:; 250:bag=1; 251:; 260:interact=1; 261:; \
280:move_x=1; 288:; 300:move_z=-1; 308:; 320:interact=1; 321:; \
340:interact=1; 341:; 360:sprint=1; 430:"
"$LOOM" sim assets/games/deeper_demo.loom --ticks 460 --hold "$CREEL_TAPE" > /tmp/loom-creel-1.json
"$LOOM" sim assets/games/deeper_demo.loom --ticks 460 --hold "$CREEL_TAPE" > /tmp/loom-creel-2.json
"$LOOM" sim assets/games/deeper_demo.loom --ticks 460 --hold "$CREEL_TAPE" > /tmp/loom-creel-3.json
cmp /tmp/loom-creel-1.json /tmp/loom-creel-2.json
cmp /tmp/loom-creel-2.json /tmp/loom-creel-3.json
rm -f /tmp/loom-creel-1.json /tmp/loom-creel-2.json /tmp/loom-creel-3.json
# ---------------------------------------------------------------------------
# **THE DECKHAND'S FOOT STAYS WHERE HE PUT IT.**
# ---------------------------------------------------------------------------
#
# **Nothing in any gate could see this animation, and that was measured before
# these rows were written.** `deckhand_walk` is in `SCENES` and not in `GOLDEN`,
# so no reference PNG contains any part of him. And the walk is invisible to
# gameplay: zeroing the ankle law leaves `Root/Walker` bit-identical —
# **x 4.456579685211182, y 0.9180648922920227 at tick 520, both ways** — because
# `character_ancestor` keeps all 28 mesh leaves out of the collision world.
# (The determinism hash *does* move, which is not a detector: it covers every
# rapier body, no scene pins it for this file, and it says only "something".)
# Thirteen scripts hang off pivots authored at +-0.001 m, and zeroing those
# millimetres puts both legs in unison with nothing anywhere reporting it.
#
# **What these rows pin is horizontal foot plant, which is the one thing four
# rounds of notes claimed was impossible to get wrong.** `deckhand_gait.rhai`
# said "phase advances with DISTANCE, so the feet cannot skate". Distance-driven
# phase makes the skate speed-INDEPENDENT; the amount is set by whether the
# stride constant matches the leg's reach, and at 2.30 rad/m it did not: the
# clock demanded a 1.37 m step from a leg that reaches 0.72 and the difference
# left as slip. The rig's own plant metric measured the sole's VERTICAL error
# only, which is how 46% horizontal slip survived underneath a number driven to
# 16 mm.
#
# Ticks 508 and 532 bracket one right-foot stance. Measured, as shipped:
# the ankle holds **4.4681 -> 4.4618** — 6.3 mm — while the body advances
# **4.1766 -> 4.7364**, 560 mm. That is 1.1% slip.
#
# **The band was chosen by falsifying it, not by picking a round number.** Each
# row below is one defect put back, alone, and the widest reading it produces in
# this window:
#
#     as shipped            4.4618 .. 4.4699     passes
#     walk_knee_down 6      4.4421 .. 4.4611     passes  <- a documented knob
#     walk_knee_down 12     4.4354 .. 4.4539     passes  <- the old value, and
#                                                           the band cannot
#                                                           separate it from 6
#     hip back to a sine    4.3963 .. 4.5320     FAILS
#     walk_leg 20 alone     4.3678 .. 4.5443     FAILS   <- see below
#     stride back to 2.30   3.9485 .. 5.0371     FAILS
#
# So 4.42 .. 4.52 is the tightest band that still lets `walk_knee_down` — the
# one knob whose whole point is to trade plant for a visibly softer knee — move
# across its documented range. **It therefore does NOT catch the stance knee on
# its own**, and that is a deliberate limit rather than an oversight: 12 degrees
# of stance flexion costs 13 points of skate and is exactly what a human might
# turn it to. Falsified in all six rows before this was written.
#
# **`walk_leg` failing alone is correct and is the row worth understanding.**
# The stride constant and the leg swing are coupled — `stride = pi / (2 * L *
# sin(leg))` — so turning the swing down without turning the stride up asks the
# clock for more ground than the leg can cover, which is exactly the defect this
# gate exists for. It reads as a surprise the first time; it is the gate
# teaching the coupling.
#
# **These literals move when the character controller does**, exactly like the
# creel's -12.308 two blocks up, and the slack is what that is for. The claim is
# "the body travelled half a metre and the planted foot did not move".
DECKHAND_ANKLE_R="Root/Walker/Body/Hips/HipR/KneeR/AnkleR"
DECKHAND_ANKLE_L="Root/Walker/Body/Hips/HipL/KneeL/AnkleL"
"$LOOM" sim assets/test/deckhand_walk.loom --ticks 508 \
  --assert "Root/Walker.x < 4.30" \
  --assert "$DECKHAND_ANKLE_R.x > 4.42" --assert "$DECKHAND_ANKLE_R.x < 4.52" >/dev/null
"$LOOM" sim assets/test/deckhand_walk.loom --ticks 532 \
  --assert "Root/Walker.x > 4.63" \
  --assert "$DECKHAND_ANKLE_R.x > 4.42" --assert "$DECKHAND_ANKLE_R.x < 4.52" >/dev/null

# **And that the two legs are half a cycle apart**, which is the check a lost
# side-detection offset on the HIP or the KNEE cannot survive. At tick 516 the
# left foot is at the top of its swing and the right is planted: 0.318 against
# 0.094. It does not cover the ankle — `deckhand_ankle.rhai` turns the boot
# about a pivot it does not move, so unison ankles are invisible to every
# assertion in this file and to `Root/Walker` as well. That one is a picture.
"$LOOM" sim assets/test/deckhand_walk.loom --ticks 516 \
  --assert "$DECKHAND_ANKLE_L.y > 0.24" --assert "$DECKHAND_ANKLE_R.y < 0.16" >/dev/null

# The mirror scene walks him into the glass and stops him there, which is the
# only scene in the project where a character is judged from the front.
"$LOOM" sim assets/test/deckhand_mirror.loom --ticks 520 \
  --assert "Root/Walker.x > 3.10" --assert "Root/Walker.y > 0.5" >/dev/null

echo "gameplay: 20 scenes asserted, 7 blocks of deliberately wrong input, the deckhand's stance foot planted, fight and demo byte-identical across 3 processes"

# ---------------------------------------------------------------------------
# 7. Work per frame. **Nothing above this line can see a frame get slower.**
#
# `loom render --frames N` used to re-bake the scene's entire voxel volume once
# per telemetry row — 4.6 s a frame on `moraine`, 119 ms on `lanternhead` — and
# no gate in this project could have told you: the pixels are identical, the
# hashes are identical, the CSV is identical. Only the clock moves, and the
# clock is the one thing a shared box makes untrustworthy.
#
# So the invariant is stated as a count instead. `loom_voxel::bakes()` counts
# `Volume::bake` calls in the process and `loom render` reports it; a scene's
# bake count must not grow with its frame count. Exact, load-invariant, and a
# stale binary cannot fake it.
#
# `cave` is the cheapest scene carrying a `VoxelVolume`: the pair below costs
# 1.3 s. Into `target/`, and into a subdirectory of it, because `--frames`
# writes `telemetry.csv` beside the frame directory rather than inside it.
#
# **No skip branch.** A render that will not run here fails this script with
# `loom`'s own error, the way the gameplay block above does. The first draft
# reported "no GPU" on a box with a 4090, because the render was in fact failing
# on a write permission — a check that skips when it cannot tell why is worth
# less than no check.
bakes_of() {
  "$LOOM" render assets/test/cave.loom --out target/gate-bakes/frames/cave.png \
    --size 64x64 --frames "$1" --spin 0 --step 0 |
    sed -n 's/.*"bakes": *\([0-9]*\).*/\1/p'
}
mkdir -p target/gate-bakes/frames
one=$(bakes_of 1)
five=$(bakes_of 5)
rm -rf target/gate-bakes
if [ "$one" != "$five" ]; then
  echo "FAIL: cave bakes $one volumes for 1 frame and $five for 5 — the render loop grew per-frame work"
  exit 1
fi
echo "per-frame work: cave bakes $one volumes at 1 frame and at 5"
