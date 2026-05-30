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
  tests/fixtures/pass/selfhost-rustc-remap.rss
)

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

printf '%s\0' "${selfhost_scripts[@]}" | xargs -0 -n1 -P "$jobs" bash -c '
  set -euo pipefail
  script="$1"
  echo "selfhost $script"
  "$RSS_BIN" run "$script"
' _
