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

# **The demo hub, and its two claims.** `assets/games/deeper_demo.loom` is where
# the fishing game is being built; `rig_walk` is that same file with one field
# changed — a reference pilot in place of the human — because `loom sim` never
# calls `set_input`.
#
# Run 1: the pilot crosses 21 m of deck and the four inventory slots fill on the
# way, which is `deeper_demo.rhai`'s proximity rule and the only detector there
# is for it — the hold lives in `GameRules` state and `state_hash` is
# physics-only, so nothing else in this file can see it.
#
# Run 2 is the one that has already failed twice. The pilot paces rail to rail
# hopping every ninety ticks with the player's own jump constants; at a 1.0 m
# rail it went over, and at 1.4 m the character controller *stepped it up onto
# the cap* and it walked off the outside. `Player.y > 2.2` is "still on the
# deck", and nothing catches a character in water, so the alternative is an
# unrecoverable fall.
"$LOOM" sim assets/test/rig_walk.loom --ticks 330 \
  --assert "Rig/Player.x > 10.0" --assert "Rig/Player.y > 2.2" \
  --assert "state.carried >= 4" --assert "events.pickup >= 4" >/dev/null

"$LOOM" sim assets/test/rig_walk.loom --ticks 1800 \
  --assert "Rig/Player.y > 2.2" --assert "state.carried >= 4" >/dev/null

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
echo "gameplay: 7 scenes asserted, fight byte-identical across 3 processes"

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
