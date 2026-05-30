#!/bin/bash
set -euo pipefail

cargo fmt --check
cargo clippy --workspace -- -D warnings

generated_manifests=(
  tests/generated/simple/Cargo.toml
  tests/generated/managed/Cargo.toml
  tests/generated/resource_pool/Cargo.toml
  tests/generated/effects/Cargo.toml
  tests/generated/result/Cargo.toml
  tests/generated/try/Cargo.toml
  tests/generated/runnable/Cargo.toml
)

for manifest in "${generated_manifests[@]}"; do
  cargo check --manifest-path "$manifest"
done

if [[ "${RSSCRIPT_FULL_TESTS:-0}" == "1" ]]; then
  cargo test --workspace
  cargo build --quiet --bin rss
  export RSS_BIN="${RSS_BIN:-$(pwd)/target/debug/rss}"
  bash scripts/lint_sources.sh
  bash scripts/run_examples.sh
  bash scripts/run_selfhost.sh
else
  cargo test --workspace --no-run
fi

git diff --check
