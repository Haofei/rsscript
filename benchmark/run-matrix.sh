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
)

json_field() {
  local json="$1"
  local field="$2"
  JSON="$json" FIELD="$field" perl -MJSON::PP -e 'my $data = decode_json($ENV{JSON}); print $data->{$ENV{FIELD}}'
}

printf '%-26s %10s %12s %12s %12s %12s %12s\n' \
  "case" "size" "eval_ms" "vm_ms" "vm_internal_ms" "release_internal_ms" "vm_int/release"
printf '%-26s %10s %12s %12s %12s %12s %12s\n' \
  "----" "----" "-------" "-----" "--------------" "-------------------" "--------------"

for entry in "${cases[@]}"; do
  case_file="${entry%%:*}"
  size="${entry##*:}"
  path="$repo_root/benchmark/$case_file"

  eval_json="$(
    "${bench_cmd[@]}" bench --json --mode eval \
      --iterations "$iterations" --warmup "$warmup" "$path" -- "$size"
  )"
  release_json="$(
    "${bench_cmd[@]}" bench --json --mode release-internal \
      --iterations "$iterations" --warmup "$warmup" "$path" -- "$size"
  )"
  vm_json="$(
    "${bench_cmd[@]}" bench --json --mode vm \
      --iterations "$iterations" --warmup "$warmup" "$path" -- "$size"
  )"
  vm_internal_json="$(
    "${bench_cmd[@]}" bench --json --mode vm-internal \
      --iterations "$iterations" --warmup "$warmup" "$path" -- "$size"
  )"

  eval_ms="$(json_field "$eval_json" mean_ms)"
  vm_ms="$(json_field "$vm_json" mean_ms)"
  vm_internal_ms="$(json_field "$vm_internal_json" mean_ms)"
  release_ms="$(json_field "$release_json" mean_ms)"
  vm_release_ratio="$(
    VM_MS="$vm_internal_ms" RELEASE_MS="$release_ms" perl -e '
      my $release = $ENV{RELEASE_MS};
      if ($release == 0) {
        print "inf";
      } else {
        printf "%.2f", $ENV{VM_MS} / $release;
      }
    '
  )"

  printf '%-26s %10s %12.3f %12.3f %14.3f %19.3f %14s\n' \
    "$case_file" "$size" "$eval_ms" "$vm_ms" "$vm_internal_ms" "$release_ms" "$vm_release_ratio"
done
