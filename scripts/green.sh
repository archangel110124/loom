#!/usr/bin/env bash
# The definition of green. All four, every time. `cargo check` passing is not green.
set -euo pipefail
cd "$(dirname "$0")/.."

./scripts/check-deps.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Vulkan validation. Needs a GPU + the layers; skips honestly without either.
if cargo metadata --no-deps --format-version 1 | grep -q '"name":"xtask"'; then
  cargo xtask validate
  # The pixel diff. Clippy catches what the compiler misses, the validation
  # layers catch what clippy misses, the determinism hashes catch a simulation
  # that drifted — and none of them notices a shader that now renders
  # everything slightly wrong. Skips honestly without a GPU, same as validate.
  cargo xtask image
  # **Byte-identity across processes** (ADR 0045 clause 3). `image` compares at
  # a calibrated tolerance, which is the right question for "did the picture
  # change" and cannot see "is the picture the same twice". Every GPU-stateful
  # path in the engine — the drop buffer, the particle pool — is licensed by
  # that second property, and until this existed it was checked by hand once.
  cargo xtask repeat
else
  echo "skip: cargo xtask validate — xtask crate does not exist yet (M2)"
fi
