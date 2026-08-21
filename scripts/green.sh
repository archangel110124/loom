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
# The whole block is under half a second. It is at the end because it needs a
# release binary, and it uses the one `cargo test` already built.
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

# **The four slots, filled by the one gesture the demo teaches.** Nothing else
# is held and nothing is aimed: W, from the spawn, past four supplies laid
# along the way to the berth. `full == 1` is the same fact from the other side
# — the capacity is a number this scene reaches by being played, which is what
# round 4's four-supplies-four-slots could never do.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 240 --hold move_z=1 \
  --assert "state.carried == 4" --assert "events.pickup >= 4" \
  --assert "state.bait == 1" --assert "state.thermos == 1" \
  --assert "state.full == 1" >/dev/null

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
"$LOOM" sim assets/test/rig_fish.loom --ticks 1800 \
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
"$LOOM" sim assets/test/rig_fish.loom --ticks 1800 \
  --assert "events.pickup >= 3" --assert "events.spend >= 1" \
  --assert "state.infish >= 1" --assert "state.carried == 4" >/dev/null

# 5c. **And bait is the reason it is an inventory and not a checklist.** The
#     control is the same scene with nothing in the hold: `fight_skilled.loom`
#     is not a hub, so `state.bait` never exists there and the unbaited
#     `BITE_MIN`/`BITE_SPAN` the five benches pin are untouched by any of this.
#     Here the demo is walked aboard holding the button: it picks up the bait
#     on the way, casts, and the take spends it.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold "move_z=1,fire=1" \
  --assert "events.spend >= 1" --assert "state.bait == 0" \
  --assert "state.carried == 3" >/dev/null

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
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1900 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1" \
  --assert "events.hooked >= 1" --assert "events.landed >= 1" \
  --assert "state.infish >= 1" --assert "state.carried == 4" >/dev/null

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
#     And the catch: 1.560 clipped to the rod when there is none, 1.846 hanging
#     in the cockpit when there is one. `state.infish` is the condition rather
#     than the phase, so it appears the tick it is landed and goes the tick it
#     is stowed — the two moments the player needs shown.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 500 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:" \
  --assert "state.infish == 0" --assert "Rig/Boat/Catch.local_y < 1.6" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1900 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1" \
  --assert "state.infish == 1" --assert "Rig/Boat/Catch.local_y > 1.7" >/dev/null
#     **And it is in his hands, which for eight rounds it was not.** The hang was
#     a fixed boat-local `(-7.40, 1.85, -3.60)`: he walked to the fish box and it
#     stayed exactly where it was, over open water, with the caption reading
#     `FISH IN HAND`. Measured at tick 1910 — `Catch` at world (-0.800, 1.878,
#     -14.056), `Player` at (-0.746, 1.607, -12.305), and the gap grows for the
#     whole walk. It now rides `state.plx/ply/plz`, which `deeper_rules.rhai`
#     solves in her frame from two of her own nodes. Ten ticks after the A of
#     the stow walk he is at boat local x -9.13 and the fish is at **-9.03**;
#     the old fixed value is -7.40, so this row is the whole difference.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1960 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1; \
1900:move_x=-1; 1950:move_z=-1" \
  --assert "state.infish == 1" --assert "Rig/Boat/Catch.local_x < -8.5" \
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
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1770 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:" \
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
#     It stows: `carried` falls 4 to 3, `infish` to 0, `stowed` to 1, and the
#     caption becomes the return-leg signpost.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2300 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1; \
1900:move_x=-1; 1950:move_z=-1" \
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
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2120 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1; \
1900:move_x=-1; 1950:move_z=-1" \
  --assert "state.stowed == 1" --assert "state.aboard == 1" \
  | grep -q '"message": "1 BELOW   the crate is 5 m behind you, on your left'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2200 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1; \
1900:move_x=-1; 1950:move_z=-1; 2000:move_z=1; 2035:move_x=1; 2060:; \
2100:move_z=1" \
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
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2100 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1; 1900:" \
  --assert "state.infish == 1" \
  | grep -q '"message": "FISH IN HAND   go LEFT round the bait box'

#     **And the other half: once he is round it, the steerable bearing is back.**
#     Ten ticks after the A the route hint is gone and the metres are counting
#     down. Without this row the route line could be latched on for ever and
#     nothing would notice; with it, both branches are pinned.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1960 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1; \
1900:move_x=-1; 1950:move_z=-1" \
  --assert "state.infish == 1" \
  | grep -q '"message": "LANDED   put her in the YELLOW FISH BOX — 3 m dead behind you"'

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
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2400 \
  --hold "0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1; 1900:move_z=-1" \
  --assert "state.stuck == 1" --assert "state.aboard == 1" \
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
#   1250  SPACE      hands off the wheel
#   1260  S + D      across the cockpit to the fish box -> STOWED at 1310
#   1320  W + A      back to the mat, which takes the helm again
#   1380  S          astern the rest of the way; alongside by 2000
#   2000  -          throttle shut, she settles
#   2060  SPACE      hands off
#   2070  S          over the boarding steps and onto the rig
#   2150  A + S      west along the wharf to the crate -> DELIVERED at 2200
#
# **Four assertions, and each is a different link in the chain.** `stow` says
# the catch went below on a walk; `deliver` says the trip closed; `stowed == 0`
# says the hold was emptied by it and not merely counted twice; `delivered == 1`
# is the number the HUD prints and the only thing in this demo that persists
# across a trip.
"$LOOM" sim assets/test/rig_trip.loom --ticks 2210 \
  --hold "0:move_z=1; 900:move_z=-1; 1250:jump=1; 1260:move_z=-1,move_x=1; \
1320:move_z=1,move_x=-1; 1380:move_z=-1; 2000:; 2060:jump=1; 2070:move_z=-1; \
2150:move_x=-1,move_z=-0.15" \
  --assert "events.landed >= 1" --assert "events.stow >= 1" \
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
"$LOOM" sim assets/test/rig_adrift.loom --ticks 2000 \
  --hold "0:; 1450:jump=1; 1460:move_z=-1,move_x=1; 1520:move_z=-1; \
1650:move_z=-1,move_x=-1; 1880:move_z=-1" \
  --assert "events.stow >= 1" --assert "state.stowed == 1" \
  --assert "events.deliver == 0" --assert "state.delivered == 0" \
  --assert "Rig/Player.y > 2.2" >/dev/null

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
DEMO_HOME="0:move_z=1; 420:jump=1; 430:; 460:fire=1; 466:; 525:fire=1; 531:; \
540:sprint=1; 580:; 630:sprint=1; 670:; 720:sprint=1; 760:; 810:sprint=1; \
850:; 900:sprint=1; 940:; 990:sprint=1; 1030:; 1080:sprint=1; 1120:; \
1170:sprint=1; 1210:; 1260:sprint=1; 1300:; 1350:sprint=1; 1390:; \
1440:sprint=1; 1480:; 1530:sprint=1; 1570:; 1620:sprint=1; 1660:; \
1710:sprint=1; 1750:; 1800:sprint=1; 1840:; 1890:sprint=1; \
1900:move_x=-1; 1950:move_z=-1; 2000:move_z=1; 2035:move_x=1; 2060:; \
2100:move_z=1; 2200:move_z=-1; 2600:; 2660:jump=1; 2670:move_z=-1; \
2750:move_x=-1,move_z=-0.15; 3550:move_z=-1,move_x=-0.3"
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2500 --hold "$DEMO_HOME" \
  --assert "state.stowed == 1" --assert "state.at_helm == 1" \
  | grep -q '"message": "THE HELM   ASTERN   wheel amidships   3 kn   SPACE lets go   ALONGSIDE — press SPACE"'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2800 --hold "$DEMO_HOME" \
  --assert "state.stowed == 1" --assert "state.at_helm == 0" \
  | grep -q '"message": "1 BELOW   SHE IS ALONGSIDE — step off, the crate is 5 m dead behind you"'
"$LOOM" sim assets/games/deeper_demo.loom --ticks 3800 --hold "$DEMO_HOME" \
  --assert "events.landed >= 1" --assert "events.deliver >= 1" \
  --assert "state.delivered == 1" --assert "state.stowed == 0" \
  | grep -q '"message": "IN THE CRATE   that is a trip. Take bait and go again"'

# 8b. **Press nothing for twenty seconds.** The one thing a stranger who has
#     read nothing will do first. He keeps the supply he spawned on top of and
#     the instruction does not decay into anything else.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 1200 --hold "0:" \
  --assert "Rig/Player.y > 2.2" --assert "state.carried == 1" \
  | grep -q '"message": "WALK FORWARD TO THE BOAT"'

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
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2000 \
  --hold "0:move_z=1; 500:move_z=1,move_x=1" \
  --assert "state.pinned == 0" --assert "state.knots > 4.0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 2000 \
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
echo "gameplay: 15 scenes asserted, 7 blocks of deliberately wrong input, fight and demo byte-identical across 3 processes"

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
