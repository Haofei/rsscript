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

for source in "${sources[@]}"; do
  echo "lint $source"
  if [[ "$source" == core/* || "$source" == tests/fixtures/pass/* ]]; then
    cargo run --quiet -- lint --no-core "$source"
  else
    cargo run --quiet -- lint "$source"
  fi
done
