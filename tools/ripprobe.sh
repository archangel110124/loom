#!/usr/bin/env bash
# The ripple grid's height and slope along +x at one tick.
# usage: tools/ripprobe.sh <scene> <tick> [max-x]
set -u
for x in $(seq 0 "${3:-7}"); do
  ./target/release/loom water "$1" --at "$x,0" --sim "$2" |
    python3 -c "
import sys,json
d=json.load(sys.stdin)['ripple']
s=d['slope']
print(f\"x=$x  height {d['height']:+.5f}  slope ({s[0]:+.4f}, {s[1]:+.4f})  |slope| {(s[0]**2+s[1]**2)**0.5:.4f}\")"
done
