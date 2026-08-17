#!/usr/bin/env bash
# Every GOLDEN row, rendered and compared — `cargo xtask image` without the
# cross-worktree singleton lock. Read-only: it never blesses.
set -u
python3 - <<'PY' > /tmp/golden_rows.txt
import re
src = open('xtask/src/main.rs').read()
start = src.index('const GOLDEN:')
body = src[start:src.index('\n];', start)]
for name, scene, args in re.findall(r'\("([\w_]+)",\s*"([^"]+)",\s*&\[([^\]]*)\]\)', body):
    print(name, scene, ' '.join(re.findall(r'"([^"]*)"', args)))
PY
while read -r name scene rest; do
  bash tools/goldcheck.sh "$name" "$scene" $rest
done < /tmp/golden_rows.txt
