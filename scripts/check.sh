#!/bin/bash
set -euo pipefail

detect_parallel_jobs() {
  local override="${1:-}"
  if [[ -n "$override" ]]; then
    echo "$override"
    return
  fi

  local cpus
  if command -v getconf >/dev/null 2>&1; then
    cpus="$(getconf _NPROCESSORS_ONLN)"
  elif command -v sysctl >/dev/null 2>&1; then
    cpus="$(sysctl -n hw.ncpu)"
  else
    cpus=4
  fi

  if ! [[ "$cpus" =~ ^[0-9]+$ ]] || (( cpus < 1 )); then
    cpus=4
  fi

  echo $((cpus * 2))
}

cargo fmt --check
cargo clippy --workspace -- -D warnings

source "$(pwd)/scripts/ramdisk_env.sh"

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
  export RUST_TEST_THREADS="${RUST_TEST_THREADS:-$(detect_parallel_jobs "${RSSCRIPT_TEST_THREADS:-}")}"
  echo "RUST_TEST_THREADS=$RUST_TEST_THREADS"
  cargo test --workspace
  if [[ "${RSSCRIPT_E2E_TESTS:-0}" == "1" ]]; then
    bash scripts/check_slow_tests.sh
  fi
  cargo build --quiet --bin rss
  export RSS_BIN="${RSS_BIN:-$(pwd)/target/debug/rss}"
  bash scripts/lint_sources.sh
  bash scripts/run_examples.sh
  bash scripts/run_selfhost.sh
else
  cargo test --workspace --no-run
fi

git diff --check
