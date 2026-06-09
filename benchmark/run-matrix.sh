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
# Build with the native-jit feature so the `jit-native` mode is available; the
# feature only adds the native tier and leaves the other modes unchanged.
bench_cmd=(cargo run --quiet --release --features native-jit --bin rss --)
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
  "selfhost_manifest_inspector.rss:benchmark/fixtures/package-medium/rsspkg.toml"
  "selfhost_stdlib_reporter.rss:0"
  "selfhost_mailbox_bench.rss:60000"
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

# Mean ms from a bench JSON blob, or "n/a" if the mode was unsupported/failed.
ms_of() {
  if [[ "$1" == \{* ]]; then json_field "$1" mean_ms; else echo "n/a"; fi
}
# Ratio of two ms strings, or "-" if either is non-numeric.
ratio_of() {
  if [[ "$1" =~ ^[0-9.]+$ && "$2" =~ ^[0-9.]+$ ]]; then ratio "$1" "$2"; else echo "-"; fi
}
# Print a ms value padded, or the raw string ("n/a") if non-numeric.
fmt_ms() {
  if [[ "$1" =~ ^[0-9.]+$ ]]; then printf '%12.3f' "$1"; else printf '%12s' "$1"; fi
}

header_fmt='%-26s %10s %12s %12s %12s %12s %10s %10s %10s\n'
# shellcheck disable=SC2059
printf "$header_fmt" \
  "case" "size" "reg_vm_ms" "jit_ms" "native_ms" "rust_ms" "reg/rust" "jit/reg" "nat/reg"
# shellcheck disable=SC2059
printf "$header_fmt" \
  "----" "----" "---------" "------" "---------" "-------" "--------" "-------" "-------"

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
  nat_json="$(
    "${bench_cmd[@]}" bench --json --mode jit-native --vm reg \
      --iterations "$iterations" --warmup "$warmup" "$path" -- "$size" 2>/dev/null || true
  )"

  release_ms="$(ms_of "$release_json")"
  reg_ms="$(ms_of "$reg_json")"
  jit_ms="$(ms_of "$jit_json")"
  native_ms="$(ms_of "$nat_json")"
  reg_release_ratio="$(ratio_of "$reg_ms" "$release_ms")"
  jit_reg_ratio="$(ratio_of "$jit_ms" "$reg_ms")"
  nat_reg_ratio="$(ratio_of "$native_ms" "$reg_ms")"

  printf '%-26s %10s ' "$case_file" "$size"
  fmt_ms "$reg_ms"; fmt_ms "$jit_ms"; fmt_ms "$native_ms"; fmt_ms "$release_ms"
  printf ' %10s %10s %10s\n' "$reg_release_ratio" "$jit_reg_ratio" "$nat_reg_ratio"
done
