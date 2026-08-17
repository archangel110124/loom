#!/usr/bin/env bash
# Water-event counts on every GOLDEN scene that floats something, at its
# blessed tick count. usage: tools/evsurvey.sh [kind ...]
set -u
kinds="${*:-submerged surfaced splash}"
for row in lanternhead:2400 river:300 water_crate:90 wake:200 splash:120 pool:400; do
  scene="assets/test/${row%%:*}.loom"
  line="${row%%:*}"
  for k in $kinds; do
    line="$line $k=$(bash tools/evcount.sh "$scene" "$k" "${row##*:}")"
  done
  echo "$line"
done
