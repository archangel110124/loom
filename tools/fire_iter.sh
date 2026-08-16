#!/usr/bin/env bash
# One render of the deck brazier plus a 1:1 crop of the flame body, for
# comparing one shader constant against another.
#
#     tools/fire_iter.sh <name>
#
# Writes `/tmp/fire_<name>.png` (3840x2160) and `/tmp/z_<name>.png` (a 350 px
# square of the flame at 2x nearest, which is where a dither's periodicity
# shows and a 4K downscale hides it).
#
# **It builds first, deliberately.** `scene.slang` is compiled into the binary
# by `build.rs`, so a stale `target/release/loom` measures the previous
# constant while reporting the current one's name.
set -euo pipefail
name="${1:?usage: tools/fire_iter.sh <name>}"
root="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"

cargo build --release --manifest-path "$root/Cargo.toml" --bin loom >&2

# The acceptance shot: `lanternhead`'s brazier moved 3.4 m east onto the lit
# deck its own scene comment says it had to be moved off. Written beside the
# original because asset paths resolve relative to the scene file.
deck="$root/assets/test/.fire_deck.loom"
sed 's/27\.6, \(.*\), 34\.0/31.0, \1, 34.0/' "$root/assets/test/lanternhead.loom" > "$deck"
"$root/target/release/loom" render "$deck" \
    --out "/tmp/fire_${name}.png" --size 3840x2160 --sim 2400 >/dev/null
rm -f "$deck"

python3 - "$name" <<'PY'
import sys
from PIL import Image
n = sys.argv[1]
Image.open(f"/tmp/fire_{n}.png").crop((2150, 900, 2500, 1250)) \
     .resize((700, 700), Image.NEAREST).save(f"/tmp/z_{n}.png")
PY
echo "/tmp/fire_${name}.png  /tmp/z_${name}.png"
