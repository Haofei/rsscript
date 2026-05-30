#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

selfhost_scripts=(
  tests/fixtures/pass/selfhost-review-classifier.rss
  tests/fixtures/pass/selfhost-package-risk.rss
  tests/fixtures/pass/selfhost-package-manifest.rss
  tests/fixtures/pass/selfhost-package-root-manifest.rss
  tests/fixtures/pass/selfhost-package-sources.rss
  tests/fixtures/pass/selfhost-package-exports.rss
  tests/fixtures/pass/selfhost-package-diff.rss
  tests/fixtures/pass/selfhost-package-lock-diff.rss
  tests/fixtures/pass/selfhost-package-manager.rss
  tests/fixtures/pass/selfhost-rustc-remap.rss
)

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
export RSSCRIPT_GENERATED_TARGET_DIR="${RSSCRIPT_GENERATED_TARGET_DIR:-$ROOT/target/rsscript-generated-target}"
export RSSCRIPT_TEMP_DIR="${RSSCRIPT_TEMP_DIR:-$ROOT/target/rsscript-temp}"
mkdir -p "$RSSCRIPT_TEMP_DIR"

printf '%s\0' "${selfhost_scripts[@]}" | xargs -0 -n1 -P "$jobs" bash -c '
  set -euo pipefail
  script="$1"
  echo "selfhost $script"
  "$RSS_BIN" run "$script"
' _
