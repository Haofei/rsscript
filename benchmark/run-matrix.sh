#!/usr/bin/env bash
set -euo pipefail

iterations=5
warmup=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --iterations)
      iterations="$2"
      shift 2
      ;;
    --warmup)
      warmup="$2"
      shift 2
      ;;
    -h|--help)
      printf 'usage: %s [--iterations N] [--warmup N]\n' "$0"
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bench_cmd=(cargo run --quiet --release --bin rss --)
cases=(
  "function_call_hot_loop.rss:200000"
  "list_closure_pipeline.rss:12000"
  "pipeline_chain.rss:16000"
  "list_index_scan.rss:19000"
  "map_insert_lookup.rss:17000"
  "map_string_keys.rss:16000"
  "sorted_map_insert.rss:8000"
  "struct_field_rw.rss:200000"
  "deque_queue.rss:50000"
)

json_field() {
  local json="$1"
  local field="$2"
  JSON="$json" FIELD="$field" perl -MJSON::PP -e 'my $data = decode_json($ENV{JSON}); print $data->{$ENV{FIELD}}'
}

ratio() {
  NUM="$1" DEN="$2" perl -e '
    my $den = $ENV{DEN};
    if ($den == 0) { print "inf"; } else { printf "%.2f", $ENV{NUM} / $den; }
  '
}

printf '%-26s %10s %12s %12s %12s %12s %10s\n' \
  "case" "size" "reg_vm_ms" "jit_ms" "rust_ms" "reg/rust" "jit/reg"
printf '%-26s %10s %12s %12s %12s %12s %10s\n' \
  "----" "----" "---------" "------" "-------" "--------" "-------"

for entry in "${cases[@]}"; do
  case_file="${entry%%:*}"
  size="${entry##*:}"
  path="$repo_root/benchmark/$case_file"

  release_json="$(
    "${bench_cmd[@]}" bench --json --mode release-internal \
      --iterations "$iterations" --warmup "$warmup" "$path" -- "$size"
  )"
  reg_json="$(
    "${bench_cmd[@]}" bench --json --mode vm-internal --vm reg \
      --iterations "$iterations" --warmup "$warmup" "$path" -- "$size" 2>/dev/null || true
  )"
  jit_json="$(
    "${bench_cmd[@]}" bench --json --mode jit-internal --vm reg \
      --iterations "$iterations" --warmup "$warmup" "$path" -- "$size" 2>/dev/null || true
  )"

  release_ms="$(json_field "$release_json" mean_ms)"
  if [[ "$reg_json" == \{* ]]; then
    reg_ms="$(json_field "$reg_json" mean_ms)"
    reg_release_ratio="$(ratio "$reg_ms" "$release_ms")"
    if [[ "$jit_json" == \{* ]]; then
      jit_ms="$(json_field "$jit_json" mean_ms)"
      jit_reg_ratio="$(ratio "$jit_ms" "$reg_ms")"
      printf '%-26s %10s %12.3f %12.3f %12.3f %12s %10s\n' \
        "$case_file" "$size" "$reg_ms" "$jit_ms" "$release_ms" "$reg_release_ratio" "$jit_reg_ratio"
    else
      printf '%-26s %10s %12.3f %12s %12.3f %12s %10s\n' \
        "$case_file" "$size" "$reg_ms" "unsupported" "$release_ms" "$reg_release_ratio" "-"
    fi
  else
    printf '%-26s %10s %12s %12s %12.3f %12s %10s\n' \
      "$case_file" "$size" "unsupported" "unsupported" "$release_ms" "-" "-"
  fi
done
