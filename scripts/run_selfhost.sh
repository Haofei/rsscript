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

for script in "${selfhost_scripts[@]}"; do
  echo "selfhost $script"
  cargo run --quiet -- run "$script"
done
