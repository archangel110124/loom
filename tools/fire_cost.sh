#!/usr/bin/env bash
# Forward-pass GPU time for the flame scenes, at the resolution CLAUDE.md quotes
# its other numbers at.
#
#     tools/fire_cost.sh [runs]
#
# **Reports the MINIMUM of N runs, deliberately.** This box shares one GPU
# between the agent's renders and whatever else is resident — `ollama ps` and a
# concurrent `cargo xtask validate` both show up as a slower frame — so the mean
# measures contention and the minimum measures the shader. N defaults to 7.
set -euo pipefail
runs="${1:-7}"
root="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
cargo build --release --manifest-path "$root/Cargo.toml" --bin loom >&2

for scene in campfire lanternhead; do
    best=""
    for _ in $(seq "$runs"); do
        t=$(LOOM_GPU_TIMING=1 "$root/target/release/loom" render \
                "$root/assets/test/$scene.loom" --out /tmp/fire_cost.png \
                --size 1920x1080 --sim 200 2>&1 \
            | grep -oP 'forward \K[0-9.]+' | head -1)
        [ -z "$t" ] && continue
        if [ -z "$best" ] || awk "BEGIN{exit !($t < $best)}"; then best="$t"; fi
    done
    echo "$scene forward: ${best:-unmeasured} ms (min of $runs, 1920x1080)"
done
