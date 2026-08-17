#!/usr/bin/env bash
# Frame-to-frame mean of a `loom render --frames` sequence.
# usage: tools/seqdiff.sh <dir> <prefix> <first-tick> [channel]
set -u
dir="$1"; pre="$2"; t0="$3"; ch="${4:-0}"
loom=./target/release/loom
n=$(ls "$dir"/"$pre"_*.png | wc -l)
for i in $(seq 0 $((n - 2))); do
  a=$(printf "%s/%s_%04d.png" "$dir" "$pre" "$i")
  b=$(printf "%s/%s_%04d.png" "$dir" "$pre" "$((i + 1))")
  m=$($loom compare "$a" "$b" --fraction 1.0 --channel "$ch" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(f"mean {d.get("mean")}  frac {d.get("fraction")}")')
  echo "$((t0 + i)) -> $((t0 + i + 1))   $m"
done
