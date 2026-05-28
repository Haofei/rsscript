#!/bin/bash
set -euo pipefail

cargo fmt --check
cargo clippy --workspace -- -D warnings

cargo check --manifest-path tests/generated/simple/Cargo.toml
cargo check --manifest-path tests/generated/managed/Cargo.toml
cargo check --manifest-path tests/generated/resource_pool/Cargo.toml
cargo check --manifest-path tests/generated/effects/Cargo.toml
cargo check --manifest-path tests/generated/result/Cargo.toml
cargo check --manifest-path tests/generated/try/Cargo.toml
cargo check --manifest-path tests/generated/runnable/Cargo.toml

if [[ "${RSSCRIPT_FULL_TESTS:-0}" == "1" ]]; then
  cargo test --workspace
  bash scripts/run_examples.sh
else
  cargo test --workspace --no-run
fi

git diff --check
