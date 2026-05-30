#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

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

cargo test -q -p rsscript-runtime json_runtime_hooks_parse_nested_fields
cargo test -q --test checker --no-run

checker_bin="$(find target/debug/deps -maxdepth 1 -type f -name 'checker-*' -perm -111 | sort | tail -1)"
if [[ -z "$checker_bin" ]]; then
  echo "checker test binary not found" >&2
  exit 1
fi

tmp_dir="$(mktemp -d "${RSSCRIPT_TEMP_DIR:-${TMPDIR:-/tmp}}/rsscript-package-manager-tdd.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

cat > "$tmp_dir/tests.txt" <<'TESTS'
rust_lowering_maps_json_core_calls_to_runtime_hooks
package_tree_expands_path_dependencies_and_marks_unresolved
rss_pkg_tree_json_reports_dependency_summary
rss_run_accepts_minimal_selfhost_package_manager_metadata
rss_run_accepts_minimal_selfhost_package_manager_vendor
rss_run_accepts_minimal_selfhost_package_manager_check
rss_run_accepts_minimal_selfhost_package_manager_tree
rss_run_accepts_minimal_selfhost_package_manager_review
TESTS

export checker_bin
export tmp_dir

printf 'package manager TDD checker gate: jobs=%s\n' "$jobs"
xargs -n1 -P "$jobs" bash -c '
  set -euo pipefail
  test_name="$1"
  safe_name="${test_name//[^A-Za-z0-9_]/_}"
  output="$tmp_dir/$safe_name.output"
  if ! "$checker_bin" --include-ignored --exact "$test_name" --quiet >"$output" 2>&1; then
    cat "$output"
    exit 1
  fi
' _ < "$tmp_dir/tests.txt"
