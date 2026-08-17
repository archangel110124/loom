#!/usr/bin/env bash
# Three fresh processes, byte-compared — `cargo xtask repeat` for one scene,
# without the cross-worktree singleton lock.
# usage: tools/repeatcheck.sh <name> <scene> [render args...]
set -u
name="$1"; scene="$2"; shift 2
for run in 1 2 3; do
  ./target/release/loom render "$scene" --out "/tmp/rep_${name}_$run.png" --size 320x200 "$@" \
    >/dev/null || { echo "$name RENDER FAILED"; exit 1; }
done
a=$(md5sum "/tmp/rep_${name}_1.png" | cut -d' ' -f1)
b=$(md5sum "/tmp/rep_${name}_2.png" | cut -d' ' -f1)
c=$(md5sum "/tmp/rep_${name}_3.png" | cut -d' ' -f1)
if [ "$a" = "$b" ] && [ "$b" = "$c" ]; then echo "$name  3/3 identical  $a"; else echo "$name  DIFFERS  $a $b $c"; fi
