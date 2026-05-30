#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

shopt -s nullglob
sources=(core/**/*.rssi examples/*.rss tests/fixtures/pass/*.rss)

if [[ "${#sources[@]}" == "0" ]]; then
  echo "no lint sources found" >&2
  exit 1
fi

detect_jobs() {
  if [[ -n "${RSSCRIPT_JOBS:-}" ]]; then
    echo "$RSSCRIPT_JOBS"
  elif command -v getconf >/dev/null 2>&1; then
    getconf _NPROCESSORS_ONLN
  elif command -v sysctl >/dev/null 2>&1; then
    sysctl -n hw.ncpu
  else
    echo 4
  fi
}

jobs="$(detect_jobs)"
if [[ -z "$jobs" || "$jobs" -lt 1 ]]; then
  jobs=1
fi

RSS_BIN="${RSS_BIN:-$ROOT/target/debug/rss}"
if [[ ! -x "$RSS_BIN" ]]; then
  cargo build --quiet --bin rss
  RSS_BIN="$ROOT/target/debug/rss"
fi
export RSS_BIN

printf '%s\0' "${sources[@]}" | xargs -0 -n1 -P "$jobs" bash -c '
  set -euo pipefail
  source="$1"
  echo "lint $source"
  if [[ "$source" == core/* || "$source" == tests/fixtures/pass/* ]]; then
    "$RSS_BIN" lint --no-core "$source"
  else
    "$RSS_BIN" lint "$source"
  fi
' _
