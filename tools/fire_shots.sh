#!/usr/bin/env bash
# The fire shot list, so every variant of the flame shader is judged on the
# same frames.
#
#     tools/fire_shots.sh <prefix> [outdir]
#
# Writes `<prefix>campfire.png`, `<prefix>lanternhead.png`, `<prefix>deck.png`
# and `<prefix>{campfire,deck}_motion_NNNN.png` into `outdir`.
#
# **It builds first, deliberately.** The flame lives in `scene.slang`, which
# `build.rs` compiles into the binary's SPIR-V — so running a stale
# `target/release/loom` measures the previous variant's shader while reporting
# the current one's name. That is the same defect as measuring an anti-aliasing
# filter on a path the filter is not on, and CLAUDE.md records what it cost.
set -euo pipefail

prefix="${1:?usage: tools/fire_shots.sh <prefix> [outdir]}"
out="${2:-/home/k-dorui/.claude/jobs/73970172/tmp/fire}"
root="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
mkdir -p "$out"

cargo build --release --manifest-path "$root/Cargo.toml" --bin loom >&2
loom="$root/target/release/loom"

# **The acceptance scene, and it has to be written HERE.** `lanternhead.loom`
# records in its own comment that the brazier was placed where the camera's
# line through it continues into the dark flank of the headland, because on the
# open deck the flame was three detached orange shards. This is that scene with
# the four brazier nodes moved 3.4 m east along the quay, onto the lit deck
# they failed against — one `sed`, nothing else touched.
#
# It is written beside the original because **asset paths resolve relative to
# the scene file**. A copy in `target/` loads with every texture missing, which
# renders a flat cream quay with no cobbles and no grass — a picture that looks
# like a lighting bug and is really a broken shot list.
deck="$root/assets/test/.fire_deck.loom"
trap 'rm -f "$deck"' EXIT
sed 's/27\.6, \(.*\), 34\.0/31.0, \1, 34.0/' "$root/assets/test/lanternhead.loom" > "$deck"

# The stills, at the resolution the human judges at. Each scene keeps the tick
# its golden reference uses, so a shot here and a `cargo xtask image` failure
# are the same picture.
"$loom" render "$root/assets/test/campfire.loom" \
    --out "$out/${prefix}campfire.png" --size 3840x2160 --sim 200
"$loom" render "$root/assets/test/lanternhead.loom" \
    --out "$out/${prefix}lanternhead.png" --size 3840x2160 --sim 2400
"$loom" render "$deck" \
    --out "$out/${prefix}deck.png" --size 3840x2160 --sim 2400

# Motion, with the camera held still so the only thing moving is the flame. A
# still cannot say whether the tongues cohere as they climb, and a pan mixes
# that question with parallax. `campfire` is the night case that must not get
# worse; `deck` is the case that has to start working. 1080p — the judgement is
# coherence across frames, and 4K only multiplies the bytes.
"$loom" render "$root/assets/test/campfire.loom" \
    --out "$out/${prefix}campfire_motion.png" --size 1920x1080 \
    --sim 200 --frames 8 --step 6 --spin 0
"$loom" render "$deck" \
    --out "$out/${prefix}deck_motion.png" --size 1920x1080 \
    --sim 2400 --frames 8 --step 6 --spin 0

# **A dolly, because a pan cannot show depth and neither can a still.**
#
# The whole claim of a line integral over a slice is that the flame has extent
# THROUGH the quad, so its interior slides against its silhouette as the eye
# moves. A pan is chosen precisely because it makes no parallax, and every shot
# above holds the camera still. `--dolly` walks the camera forward, which is the
# only motion that can falsify the claim: if the fire were still a slice, its
# interior would translate rigidly with its outline.
#
# **`--step 1`, and NOT `--step 0`, which renders nothing.** Particles are
# stepped after a frame is written, so a zero step means they are never stepped
# at all and every frame comes out with no fire in it — the same pre-existing
# defect that makes frame 0000 of every motion sequence in this job empty. One
# tick is 1/60 s, over which the flame barely moves, so what changes across
# these frames is essentially the viewpoint alone.
"$loom" render "$deck" \
    --out "$out/${prefix}deck_dolly.png" --size 1920x1080 \
    --sim 2400 --frames 8 --step 1 --spin 0 --dolly 0.35

echo "wrote $(ls "$out/$prefix"*.png | wc -l) shots to $out with prefix '$prefix'"
