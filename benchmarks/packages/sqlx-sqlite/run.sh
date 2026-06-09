#!/usr/bin/env bash
# sqlx + SQLite benchmark matrix: runs the same workload through the RSS register
# VM, the RSS->Rust compiled backend, and a hand-written native Rust baseline.
set -euo pipefail

size=500
rows=200
iterations=5
warmup=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --size) size="$2"; shift 2 ;;
    --rows) rows="$2"; shift 2 ;;
    --iterations) iterations="$2"; shift 2 ;;
    --warmup) warmup="$2"; shift 2 ;;
    -h|--help)
      printf 'usage: %s [--size N] [--rows N] [--iterations N] [--warmup N]\n' "$0"
      printf '  --size  number of queries per timed run\n'
      printf '  --rows  rows returned by each query\n'
      exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

bench_pkg="benchmarks/packages/sqlx-sqlite"
native_manifest="$bench_pkg/native/Cargo.toml"

echo "building rss + native baseline (first run also builds the VM shim cdylib on demand)..."
cargo build --release --quiet --bin rss
cargo build --release --quiet --manifest-path "$native_manifest"

rss_bin="target/release/rss"
native_bin="$bench_pkg/native/target/release/native_baseline"

printf '\nsize=%s rows=%s iterations=%s warmup=%s\n\n' "$size" "$rows" "$iterations" "$warmup"

# Native Rust baseline.
"$native_bin" "$size" "$rows" "$iterations" "$warmup"

# RSS compiled (lowered to Rust, links the sqlx native crate).
"$rss_bin" bench --mode release-internal --iterations "$iterations" --warmup "$warmup" \
  "$bench_pkg" -- "$size" "$rows"

# RSS register VM (native bindings dynamically loaded via the generated shim).
"$rss_bin" bench --mode vm-internal --iterations "$iterations" --warmup "$warmup" \
  "$bench_pkg" -- "$size" "$rows"
