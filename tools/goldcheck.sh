#!/usr/bin/env bash
# Render one GOLDEN scene at its blessed arguments and compare it to its
# reference at the gate's own default tolerance. The same two commands
# `cargo xtask image` runs, minus the cross-worktree lock.
# usage: tools/goldcheck.sh <name> <scene> [render args...]
set -u
name="$1"; scene="$2"; shift 2
out="/tmp/gold_$name.png"
./target/release/loom render "$scene" --out "$out" --size 320x200 "$@" >/dev/null || { echo "$name RENDER FAILED"; exit 1; }
if ./target/release/loom compare "$out" "tests/references/$name.png" >/tmp/gold_"$name".json; then
  echo "$name  match"
else
  echo "$name  MOVED  $(tr -d '\n' </tmp/gold_"$name".json)"
fi
