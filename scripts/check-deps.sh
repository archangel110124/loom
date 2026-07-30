#!/usr/bin/env bash
# Dependency rules from LOOM-BUILD-BRIEF.md §3 / CLAUDE.md. Brief §7.6: violating
# these is how the project becomes unbuildable in month four.
#
# The brief says "loom_reflect and loom_scene depend on nothing else in the
# workspace". Read literally that forbids loom_scene -> loom_reflect, which would
# make schema validation on load impossible. Design doc §2.13 adds "everything
# else may depend on them" and marks loom_reflect "build first", so the intent is
# that these two are LEAVES, not that they are mutually isolated. Encoded here as:
# loom_scene may depend on loom_reflect and nothing else in-workspace.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
ws_deps() { cargo tree -p "$1" --depth 1 --prefix none 2>/dev/null | tail -n +2 | grep -oE '^loom_[a-z_]+' || true; }
has() { cargo metadata --no-deps --format-version 1 | grep -q "\"name\":\"$1\""; }

# loom_reflect: no in-workspace dependencies at all.
if has loom_reflect; then
  for d in $(ws_deps loom_reflect); do
    echo "FAIL: loom_reflect depends on $d — it must depend on nothing in-workspace"; fail=1
  done
fi

# loom_scene: loom_reflect only.
if has loom_scene; then
  for d in $(ws_deps loom_scene); do
    [ "$d" = "loom_reflect" ] && continue
    echo "FAIL: loom_scene depends on $d — only loom_reflect is permitted"; fail=1
  done
fi

# loom_agent: depended on by nothing. The agent layer must be removable, or you
# cannot tell whether a bug is in the engine or the agent (design doc §2.13).
if has loom_agent; then
  for c in $(cargo metadata --no-deps --format-version 1 | grep -oE '"name":"loom_[a-z_]+"' | grep -oE 'loom_[a-z_]+'); do
    [ "$c" = "loom_agent" ] && continue
    if ws_deps "$c" | grep -qx loom_agent; then
      echo "FAIL: $c depends on loom_agent — nothing may depend on it"; fail=1
    fi
  done
fi

# ash containment: nothing outside loom_render* may import it.
while IFS= read -r f; do
  case "$f" in crates/loom_render*/*) continue ;; esac
  if grep -qE '^\s*(use|extern crate)\s+ash\b' "$f"; then
    echo "FAIL: $f imports ash outside loom_render*"; fail=1
  fi
done < <(find crates -name '*.rs' 2>/dev/null)

[ $fail -eq 0 ] && echo "dependency rules: ok"
exit $fail
