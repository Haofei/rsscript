#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cleanup_example_artifacts() {
  rm -f \
    rsscript-buffer-consume.txt \
    rsscript-cache-image-input.bin \
    rsscript-cache-image-output.bin \
    rsscript-config-first.txt \
    rsscript-config-second.txt \
    rsscript-csv-example.csv \
    rsscript-file-copy-input.txt \
    rsscript-file-copy-output.txt \
    rsscript-file-copy-buffer-input.txt \
    rsscript-file-copy-buffer-output.txt \
    rsscript-image-input.bin \
    rsscript-image-output.bin \
    rsscript-rules-first.txt \
    rsscript-rules-second.txt
  rm -rf rsscript-file-copy-output
}

cleanup_example_artifacts
trap cleanup_example_artifacts EXIT

shopt -s nullglob
examples=(examples/*.rss)

if [[ "${#examples[@]}" == "0" ]]; then
  echo "no examples found" >&2
  exit 1
fi

detect_jobs() {
  if [[ -n "${RSSCRIPT_JOBS:-}" ]]; then
    echo "$RSSCRIPT_JOBS"
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

jobs="$(detect_jobs)"
if ! [[ "$jobs" =~ ^[0-9]+$ ]] || (( jobs < 1 )); then
  jobs=1
fi

RSS_BIN="${RSS_BIN:-$ROOT/target/debug/rss}"
if [[ ! -x "$RSS_BIN" ]]; then
  cargo build --quiet --bin rss
  RSS_BIN="$ROOT/target/debug/rss"
fi
export RSS_BIN
source "$ROOT/scripts/ramdisk_env.sh"

printf '%s\0' "${examples[@]}" | xargs -0 -n1 -P "$jobs" bash -c '
  set -euo pipefail
  example="$1"
  echo "run $example"
  "$RSS_BIN" run "$example"
' _
