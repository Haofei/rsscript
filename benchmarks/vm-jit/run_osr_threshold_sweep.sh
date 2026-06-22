#!/usr/bin/env bash
# OSR backedge-threshold sweep driver (jit-native auto-trigger path).
# Runs inside the dev container. Emits TSV: trip threshold iters median_ms osr_entries
set -u
KERNEL=benchmarks/vm-jit/kernels/osr_scalar_loop.rss
BIN=./target/release/rss

trips=(500 1000 1500 2000 3000 5000 10000 50000)
thresholds=(0 100 250 500 1000 2000 5000)

echo -e "trip\tthreshold\titers\tmedian_ms\tosr_entries"
for trip in "${trips[@]}"; do
  # Short loops are fast: use many iterations to beat timer noise.
  if   [ "$trip" -le 2000 ];  then iters=201; warmup=20
  elif [ "$trip" -le 10000 ]; then iters=101; warmup=10
  else                              iters=41;  warmup=5
  fi
  for t in "${thresholds[@]}"; do
    out=$(RSS_JIT_OSR_THRESHOLD="$t" $BIN bench --json --mode jit-native \
          --iterations "$iters" --warmup "$warmup" "$KERNEL" -- "$trip" 2>/dev/null)
    median=$(echo "$out" | grep -o '"median_ms":[0-9.]*' | head -1 | cut -d: -f2)
    osr=$(echo "$out" | grep -o '"osr_entries":[0-9]*' | head -1 | cut -d: -f2)
    echo -e "${trip}\t${t}\t${iters}\t${median}\t${osr}"
  done
done
