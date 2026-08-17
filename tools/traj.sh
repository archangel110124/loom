#!/usr/bin/env bash
# Sphere height per tick, via a deliberately-failing assertion's `actual`.
# usage: tools/traj.sh <scene> <node.axis> <from> <to> [stride]
set -u
scene="$1"; probe="$2"; from="$3"; to="$4"; stride="${5:-1}"
for t in $(seq "$from" "$stride" "$to"); do
  v=$(./target/release/loom sim "$scene" --ticks "$t" --assert "$probe > 1e30" |
      python3 -c 'import sys,json;d=json.load(sys.stdin);print(d["failed_assertions"][0]["actual"])')
  echo "$t $v"
done
