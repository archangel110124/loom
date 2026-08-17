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

# **Prove the manifests parse before trusting a single rule below.** Every
# check here is gated on `has`, which is a grep over `cargo metadata` — so a
# manifest that fails to load makes `has` false for every crate, skips every
# stanza, and prints "dependency rules: ok" having checked nothing. That is
# the one failure mode this file must not have.
if ! cargo metadata --no-deps --format-version 1 >/dev/null 2>&1; then
  echo "FAIL: cargo metadata does not load — no dependency rule below was checked"
  cargo metadata --no-deps --format-version 1 >/dev/null
  exit 1
fi

# The `|| true` keeps a crate with no in-workspace deps from tripping `set -e`,
# but it also used to swallow `cargo tree` *failing* — and a cyclic dependency
# makes it fail, which is precisely the violation these rules exist to catch.
# So the failure is separated from the empty result and reported.
ws_deps() {
  local out
  if ! out=$(cargo tree -p "$1" --depth 1 --prefix none 2>&1); then
    echo "FAIL: cargo tree -p $1 failed — the graph is broken, likely a cycle" >&2
    echo "$out" >&2
    return 1
  fi
  printf '%s\n' "$out" | tail -n +2 | grep -oE '^loom_[a-z_]+' || true
}
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

# loom_editor: depended on by nothing but loom_cli. Same shape as the loom_agent
# rule above. "Stripping the editor" means not linking this crate (ADR 0022) —
# egui stays unconditional in loom_render because the HUD is drawn with it, so
# the removability guarantee rests entirely on this edge.
if has loom_editor; then
  for c in $(cargo metadata --no-deps --format-version 1 | grep -oE '"name":"loom_[a-z_]+"' | grep -oE 'loom_[a-z_]+'); do
    case "$c" in loom_editor|loom_cli) continue ;; esac
    if ws_deps "$c" | grep -qx loom_editor; then
      echo "FAIL: $c depends on loom_editor — only loom_cli may"; fail=1
    fi
  done

  # The other half: the editor must actually drop out without its feature.
  # This checks the dependency *edge*, which is what containment means; that
  # `--no-default-features` also compiles is Stage 5's exit criterion, because
  # run.rs is not split until then.
  for forbidden in loom_editor egui_dock; do
    if cargo tree -p loom_cli --no-default-features -e normal 2>/dev/null | grep -qE "^[^a-z]*$forbidden v"; then
      echo "FAIL: loom_cli --no-default-features still pulls in $forbidden"; fail=1
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
