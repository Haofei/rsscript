#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

dogfood_scripts=(
  tests/fixtures/pass/dogfood-review-classifier.rss
  tests/fixtures/pass/dogfood-package-risk.rss
  tests/fixtures/pass/dogfood-package-exports.rss
  tests/fixtures/pass/dogfood-package-diff.rss
  tests/fixtures/pass/dogfood-package-lock-diff.rss
  tests/fixtures/pass/dogfood-rustc-remap.rss
)

for script in "${dogfood_scripts[@]}"; do
  echo "dogfood $script"
  cargo run --quiet -- run "$script"
done
