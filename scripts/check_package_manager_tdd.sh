#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export RSSCRIPT_GENERATED_TARGET_DIR="${RSSCRIPT_GENERATED_TARGET_DIR:-$ROOT/target/rsscript-generated-target}"
export RSSCRIPT_TEMP_DIR="${RSSCRIPT_TEMP_DIR:-$ROOT/target/rsscript-temp}"
mkdir -p "$RSSCRIPT_TEMP_DIR"

cargo test -q --test checker
cargo test -q --test checker rss_run_accepts_minimal_selfhost_package_manager_check -- --include-ignored --exact
