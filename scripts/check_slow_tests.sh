#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

threshold="${RSSCRIPT_SLOW_TEST_THRESHOLD:-10}"
pattern="${RSSCRIPT_SLOW_TEST_PATTERN:-.*}"
source "$ROOT/scripts/ramdisk_env.sh"

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

  if (( cpus > 4 )); then
    cpus=4
  fi

  echo "$cpus"
}

jobs="$(detect_jobs)"
if ! [[ "$jobs" =~ ^[0-9]+$ ]] || (( jobs < 1 )); then
  jobs=1
fi

cargo test --test checker --no-run
checker_bin="$(find target/debug/deps -maxdepth 1 -type f -name 'checker-*' -perm -111 | sort | tail -1)"
if [[ -z "$checker_bin" ]]; then
  echo "checker test binary not found" >&2
  exit 1
fi

tmp_dir="$(mktemp -d "${RSSCRIPT_TEMP_DIR:-${TMPDIR:-/tmp}}/rsscript-slow-tests.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

"$checker_bin" --list | awk -F: -v pattern="$pattern" '$2 ~ /test/ && $1 ~ pattern {print $1}' \
  > "$tmp_dir/tests.txt"

if [[ ! -s "$tmp_dir/tests.txt" ]]; then
  echo "no checker tests matched pattern: $pattern" >&2
  exit 1
fi

export checker_bin
export threshold
export tmp_dir

printf 'checker test duration gate: jobs=%s threshold=%ss pattern=%s\n' "$jobs" "$threshold" "$pattern"

xargs -n1 -P "$jobs" bash -c '
  set -euo pipefail
  test_name="$1"
  safe_name="${test_name//[^A-Za-z0-9_]/_}"
  output="$tmp_dir/$safe_name.output"
  start="$(perl -MTime::HiRes=time -e "printf qq(%.6f\\n), time")"
  if ! "$checker_bin" --include-ignored --exact "$test_name" --quiet >"$output" 2>&1; then
    cat "$output"
    exit 1
  fi
  end="$(perl -MTime::HiRes=time -e "printf qq(%.6f\\n), time")"
  elapsed="$(awk -v start="$start" -v end="$end" "BEGIN {printf \"%.2f\", end - start}")"
  printf "%6ss %s\n" "$elapsed" "$test_name" > "$tmp_dir/$safe_name.time"
' _ < "$tmp_dir/tests.txt"

cat "$tmp_dir"/*.time | sort -nr | tee "$tmp_dir/times.txt"

if awk -v threshold="$threshold" '$1 + 0 > threshold {found = 1} END {exit found ? 0 : 1}' "$tmp_dir/times.txt"; then
  echo "checker tests over ${threshold}s; refactor, split, or move expensive work out of the test:" >&2
  awk -v threshold="$threshold" '$1 + 0 > threshold {print}' "$tmp_dir/times.txt" >&2
  exit 1
fi

echo "checker test duration gate passed: no matched test exceeded ${threshold}s"
