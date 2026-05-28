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
}

cleanup_example_artifacts
trap cleanup_example_artifacts EXIT

shopt -s nullglob
examples=(examples/*.rss)

if [[ "${#examples[@]}" == "0" ]]; then
  echo "no examples found" >&2
  exit 1
fi

for example in "${examples[@]}"; do
  echo "run $example"
  cargo run --quiet -- run "$example"
done
