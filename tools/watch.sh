#!/usr/bin/env bash
#
# Capture a scene in motion and tile it into one labelled contact sheet.
#
# A single image costs about the same in vision tokens whatever its resolution,
# so twenty frames on one sheet cost roughly what one screenshot does. That is
# the whole point: motion bugs — a phase error, a sign flip, a lag, a
# discontinuity — are invisible in a still and obvious across a row of frames.
#
#   tools/watch.sh homestead
#   FRAMES=9 GRID=3x3 CELL_W=480 tools/watch.sh meadow    # detail
#   STEP=40 tools/watch.sh ocean                          # slower motion
#
# The engine already dumps deterministic numbered frames offscreen; this script
# only drives it and tiles the result.
set -euo pipefail

SCENE="${1:-homestead}"
FRAMES="${FRAMES:-20}"
GRID="${GRID:-4x5}"
CELL_W="${CELL_W:-320}"
# Simulated ticks between captured frames, at the fixed 60 Hz timestep. This is
# the knob the brief called `--dt`: Loom's timestep is a locked decision and is
# never variable, so what a capture chooses is how far apart its samples are.
STEP="${STEP:-6}"
# Ticks to simulate before the first frame — the brief's `--warmup`.
WARMUP="${WARMUP:-0}"
SIZE="${SIZE:-960x540}"
OUT="${OUT:-target/agent}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# A scene may be given by name or by path.
if [ -f "$SCENE" ]; then
  scene_path="$SCENE"
elif [ -f "assets/test/$SCENE.loom" ]; then
  scene_path="assets/test/$SCENE.loom"
elif [ -f "assets/games/$SCENE.loom" ]; then
  scene_path="assets/games/$SCENE.loom"
else
  echo "watch: no scene '$SCENE'." >&2
  echo "  tried: $SCENE, assets/test/$SCENE.loom, assets/games/$SCENE.loom" >&2
  echo "  available:" >&2
  ls assets/test/*.loom assets/games/*.loom 2>/dev/null | sed 's/^/    /' >&2
  exit 2
fi

have_ffmpeg=0
if command -v ffmpeg >/dev/null 2>&1; then have_ffmpeg=1; fi
have_montage=0
if command -v montage >/dev/null 2>&1; then have_montage=1; fi
if [ "$have_ffmpeg" -eq 0 ] && [ "$have_montage" -eq 0 ]; then
  echo "watch: needs ffmpeg or ImageMagick to tile the frames." >&2
  echo "  sudo dnf install ffmpeg      # or: sudo dnf install ImageMagick" >&2
  exit 2
fi

mkdir -p "$OUT/frames"
rm -f "$OUT/frames/"frame_*.png "$OUT/contact_sheet.png"

# **The first few frames of a rain scene are not reproducible**, and this says
# so rather than letting two captures of the same scene quietly disagree.
#
# Measured on `rain_impact` with no warmup: frames 0-3 differ between runs by
# 5268, 20367, 9766 and 578 pixels, and every frame from 4 on is bit-identical.
# The drop layer is stateful and GPU-resident (ADR 0017); its opening frames
# depend on state that is not reproducibly seeded, and it washes out within
# about four frames as drops are re-issued. `WARMUP=300` is clean.
#
# `meadow` — grass and wind, no rain — is bit-identical from frame 0.
if grep -q '\[node.components.Rain\]' "$scene_path" 2>/dev/null && [ "$WARMUP" -lt 240 ]; then
  echo "watch: $SCENE has rain and WARMUP=$WARMUP; the first ~4 frames will not" >&2
  echo "       reproduce between runs. Use WARMUP=300 when comparing two captures." >&2
fi

echo "watch: $scene_path — $FRAMES frames, $STEP ticks apart, $SIZE"
cargo run --release -q -p loom_cli -- render "$scene_path" \
  --out "$OUT/frames/frame.png" \
  --size "$SIZE" \
  --frames "$FRAMES" \
  --step "$STEP" \
  --sim "$WARMUP" >/dev/null

count=$(ls "$OUT/frames"/frame_*.png 2>/dev/null | wc -l)
if [ "$count" -ne "$FRAMES" ]; then
  echo "watch: expected $FRAMES frames, found $count" >&2
  exit 1
fi

# **The numbers are the deliverable, not decoration.** They are what lets a
# finding be stated as "the sign flips at frame 12" instead of "somewhere in the
# middle", so an unlabelled sheet is a failure rather than a lesser success.
#
# **Scale first, then label.** Drawing at full frame size and scaling afterwards
# shrinks a 22 px number to seven, which is present in the file and unreadable
# on the sheet — which is the same thing as absent, and worse because it looks
# like it worked. The first version of this script did exactly that.
# **Each frame is labelled on its own, with its own literal index**, rather
# than by ffmpeg's `%{n}` inside a `scale,drawtext,tile` chain. That chain is
# the obvious one and it *exits 0 while drawing nothing* — a tiled sheet with
# no numbers on it, which is the exact failure this section is written to
# prevent and which took two attempts to notice. A per-frame label cannot fail
# quietly: if the annotation is missing the file is missing.
labelled="$OUT/labelled"
rm -rf "$labelled"
mkdir -p "$labelled"
i=0
for frame in "$OUT/frames"/frame_*.png; do
  magick "$frame" -resize "${CELL_W}x" \
    -fill '#ffe000' -undercolor '#000000c0' -pointsize 22 -gravity NorthWest \
    -annotate +5+4 " $i " \
    "$labelled/$(printf 'cell_%04d.png' "$i")"
  i=$((i + 1))
done

montage "$labelled"/cell_*.png \
  -tile "$GRID" -geometry '+3+3' -background '#111111' \
  "$OUT/contact_sheet.png"

# Proof the labels are there, rather than trust. A sheet with no yellow on it
# is an unlabelled sheet whatever the commands returned.
if ! magick "$OUT/contact_sheet.png" -fuzz 12% -fill black +opaque '#ffe000' \
     -format '%[fx:mean]' info: 2>/dev/null | awk '{ exit ($1 > 0.00002) ? 0 : 1 }'; then
  echo "watch: the sheet has no legible frame numbers on it — refusing to pretend" >&2
  exit 1
fi
rm -rf "$labelled"

# Keep the sheet inside what a vision model will actually read: 8000 px on any
# edge, and under 5 MB.
#
# **Quantised rather than scaled down when it is too large.** A noisy scene —
# rain especially — makes a big PNG out of high-frequency detail, and shrinking
# the cells costs exactly the legibility the sheet exists for. Dropping to 256
# colours costs almost nothing at this cell size and typically halves the file.
if command -v magick >/dev/null 2>&1; then
  # 8-bit: the tiler can emit 16-bit, which doubles the file for a contact
  # sheet nobody will colour-grade.
  magick "$OUT/contact_sheet.png" -depth 8 -resize '8000x8000>' "$OUT/contact_sheet.png"
  for colors in 256 128 64; do
    bytes=$(stat -c%s "$OUT/contact_sheet.png")
    if [ "$bytes" -le 5000000 ]; then break; fi
    magick "$OUT/contact_sheet.png" -colors "$colors" "$OUT/contact_sheet.png"
  done
fi

bytes=$(stat -c%s "$OUT/contact_sheet.png")
if [ "$bytes" -gt 5000000 ]; then
  echo "watch: sheet is $((bytes / 1024)) KB, over the 5 MB budget — try FRAMES=9 GRID=3x3" >&2
fi
echo "wrote $OUT/contact_sheet.png ($FRAMES frames, $GRID grid, $((bytes / 1024)) KB)"
if [ -f "$OUT/telemetry.csv" ]; then
  echo "wrote $OUT/telemetry.csv ($(( $(wc -l < "$OUT/telemetry.csv") - 1 )) rows)"
else
  echo "watch: no telemetry.csv — the scene has no probed systems" >&2
fi
