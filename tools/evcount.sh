#!/usr/bin/env bash
# Count of one event kind at a tick, via a deliberately-failing assertion.
# usage: tools/evcount.sh <scene> <kind> <ticks>
set -u
./target/release/loom sim "$1" --ticks "$3" --assert "events.$2 > 1e30" |
  python3 -c 'import sys,json;d=json.load(sys.stdin);print(d["failed_assertions"][0]["actual"])'
