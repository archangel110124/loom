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

# The hub is crossable, and a lap of it fills the four inventory slots.
"$LOOM" sim assets/test/rig_walk.loom --ticks 220 \
  --assert "Rig/Player.x > 10.0" --assert "Rig/Player.y > 2.2" >/dev/null
"$LOOM" sim assets/test/rig_walk.loom --ticks 500 \
  --assert "state.carried >= 4" --assert "events.pickup >= 4" \
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
  --assert "Rig/Player.y > -1.0" --assert "state.carried >= 4" >/dev/null

# **The demo in one key.** From the spawn, holding W: past a supply, through the
# gap in the berth railing, over the bulwark, onto the cockpit deck, onto the
# helm mat, and away. `z < -11.0` is inboard of her port side deck; the y band
# is "standing on the deck" against 2.55 on the bulwark cap and -0.70 in the sea.
#
# **`state.at_helm` is the claim `Rig/Player.z < -11.0` was standing in for.**
# A position band is satisfied by a player standing anywhere in a strip of
# cockpit, and by a boat that drifted under one; `deeper_demo.rhai` computes it
# from the *drawn* mat's world position, so it follows her when she turns and
# it is the same fact the HUD line prints. Run 1 pairs it with tick 60, where
# the answer is still no.
"$LOOM" sim assets/games/deeper_demo.loom --ticks 60 --hold move_z=1 \
  --assert "state.at_helm == 0" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 240 --hold move_z=1 \
  --assert "Rig/Player.z < -11.0" --assert "Rig/Player.y > 1.2" \
  --assert "Rig/Player.y < 1.9" --assert "state.carried >= 1" \
  --assert "state.at_helm == 1" >/dev/null
"$LOOM" sim assets/games/deeper_demo.loom --ticks 900 --hold move_z=1 \
  --assert "Rig/Boat.x > 30.0" --assert "Rig/Player.y > 1.2" \
  --assert "Rig/Boat.y > -0.4" >/dev/null

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
"$LOOM" sim assets/test/rig_drive.loom --ticks 900 --hold "move_z=1,move_x=1" \
  --assert "Rig/Boat.z > -20.0" --assert "Rig/Boat.x > 20.0" \
  --assert "Rig/Player.y > 1.2" >/dev/null
"$LOOM" sim assets/test/rig_drive.loom --ticks 900 --hold "move_z=1,move_x=-1" \
  --assert "Rig/Boat.z < -60.0" --assert "Rig/Boat.x > 20.0" \
  --assert "Rig/Player.y > 1.2" >/dev/null

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
# sank at all. `-1.0 < y < 0.0` is floating, not standing, not drowning.
"$LOOM" sim assets/test/rig_overboard.loom --ticks 240 \
  --assert "Rig/Player.y > -1.0" --assert "Rig/Player.y < 0.0" >/dev/null
"$LOOM" sim assets/test/rig_overboard.loom --ticks 900 --hold move_x=-1 \
  --assert "Rig/Player.y > 2.2" --assert "Rig/Player.x < 11.5" >/dev/null

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
echo "gameplay: 11 scenes asserted, fight byte-identical across 3 processes"

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
