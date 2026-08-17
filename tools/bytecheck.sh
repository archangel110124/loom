#!/usr/bin/env bash
# Byte-compare the last `tools/goldcheck.sh` render of each named scene against
# its reference. `loom compare` has a tolerance; this is the question that has
# none — "is a scene without the feature bit for bit what it was".
# usage: tools/bytecheck.sh <name>...
set -u
for n in "$@"; do
  a=$(md5sum "/tmp/gold_$n.png" | cut -d' ' -f1)
  b=$(md5sum "tests/references/$n.png" | cut -d' ' -f1)
  if [ "$a" = "$b" ]; then echo "$n  byte-identical"; else echo "$n  DIFFERS  $a  $b"; fi
done
