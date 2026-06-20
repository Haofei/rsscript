#!/usr/bin/env bash
# Capture the VM/JIT performance baseline across every slow-path kernel.
#
# Runs each case in `cases.tsv` through four execution modes and records mean
# milliseconds for each, plus the tier ratios that the perf plan tracks:
#   reg_vm  : register VM, no JIT          (--mode vm-internal --vm reg)
#   jit     : tier-0 in-process JIT        (--mode jit-internal --vm reg)
#   native  : Cranelift native tier        (--mode jit-native   --vm reg)
#   rust    : generated release Rust       (--mode release-internal)
#
# Output: a human-readable table on stdout AND a machine-readable JSON array
# written to benchmarks/vm-jit/baseline/baseline-<date>.json (override with
# --out PATH). The JSON is the committed baseline the perf plan compares against.
#
# Run inside the dev container (per repo convention), e.g.:
#   docker compose run --rm dev ./benchmarks/vm-jit/run-baseline.sh
set -euo pipefail

iterations=5
warmup=1
out=""
# Per-mode wall-clock cap. A pathological kernel (e.g. a super-linear runtime
# path) must degrade to an `n/a` cell, never hang the whole suite. Override with
# --timeout 0 to disable.
mode_timeout=180

while [[ $# -gt 0 ]]; do
  case "$1" in
    --iterations) iterations="$2"; shift 2 ;;
    --warmup)     warmup="$2";     shift 2 ;;
    --timeout)    mode_timeout="$2"; shift 2 ;;
    --out)        out="$2";        shift 2 ;;
    -h|--help)
      printf 'usage: %s [--iterations N] [--warmup N] [--timeout SECS] [--out PATH]\n' "$0"
      exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
cases_file="$script_dir/cases.tsv"

if [[ -z "$out" ]]; then
  mkdir -p "$script_dir/baseline"
  out="$script_dir/baseline/baseline-$(date +%Y%m%d).json"
fi

# Build once with the native-jit feature so the jit-native mode exists; other
# modes are unaffected by the feature flag.
bench_cmd=(cargo run --quiet --release --features native-jit --bin rss --)

json_field() {
  JSON="$1" FIELD="$2" perl -MJSON::PP -e \
    'my $d = decode_json($ENV{JSON}); print $d->{$ENV{FIELD}} // "";'
}
# Mean ms from a bench JSON blob, or "n/a" if the mode was unsupported/failed.
ms_of() { if [[ "$1" == \{* ]]; then json_field "$1" mean_ms; else echo "n/a"; fi; }
ratio_of() {
  if [[ "$1" =~ ^[0-9.]+$ && "$2" =~ ^[0-9.]+$ && "$2" != "0" ]]; then
    NUM="$1" DEN="$2" perl -e 'printf "%.2f", $ENV{NUM} / $ENV{DEN};'
  else echo "-"; fi
}
fmt_ms() { if [[ "$1" =~ ^[0-9.]+$ ]]; then printf '%11.3f' "$1"; else printf '%11s' "$1"; fi; }
# Emit a numeric value or JSON null for non-numeric ("n/a") cells.
jnum() { if [[ "$1" =~ ^[0-9.]+$ ]]; then printf '%s' "$1"; else printf 'null'; fi; }

run_mode() { # mode, path, size  ->  bench JSON or empty (empty => prints n/a)
  local guard=()
  [[ "$mode_timeout" != "0" ]] && guard=(timeout "$mode_timeout")
  "${guard[@]}" "${bench_cmd[@]}" bench --json --mode "$1" "${@:4}" \
    --iterations "$iterations" --warmup "$warmup" "$2" -- "$3" 2>/dev/null || true
}

header='%-20s %-22s %10s %11s %11s %11s %11s %9s %9s %9s\n'
# shellcheck disable=SC2059
printf "$header" category case size reg_vm_ms jit_ms native_ms rust_ms reg/rust jit/reg nat/reg
# shellcheck disable=SC2059
printf "$header" -------- ---- ---- --------- ------ --------- ------- -------- ------- -------

json_rows=()

# Fields are whitespace-separated; category/path/size are single tokens and the
# trailing slow-path tag (which may contain spaces) is slurped into `_tag`.
while read -r category path size _tag; do
  [[ -z "${category// }" || "${category:0:1}" == "#" ]] && continue
  case_file="$(basename "$path")"
  abs="$repo_root/$path"

  release_json="$(run_mode release-internal "$abs" "$size")"
  reg_json="$(run_mode vm-internal "$abs" "$size" --vm reg)"
  jit_json="$(run_mode jit-internal "$abs" "$size" --vm reg)"
  nat_json="$(run_mode jit-native "$abs" "$size" --vm reg)"

  reg_ms="$(ms_of "$reg_json")";    jit_ms="$(ms_of "$jit_json")"
  native_ms="$(ms_of "$nat_json")"; rust_ms="$(ms_of "$release_json")"

  printf '%-20s %-22s %10s ' "$category" "$case_file" "$size"
  fmt_ms "$reg_ms"; fmt_ms "$jit_ms"; fmt_ms "$native_ms"; fmt_ms "$rust_ms"
  printf ' %9s %9s %9s\n' \
    "$(ratio_of "$reg_ms" "$rust_ms")" \
    "$(ratio_of "$jit_ms" "$reg_ms")" \
    "$(ratio_of "$native_ms" "$reg_ms")"

  json_rows+=("$(printf '{"category":"%s","case":"%s","size":"%s","reg_vm_ms":%s,"jit_ms":%s,"native_ms":%s,"rust_ms":%s}' \
    "$category" "$case_file" "$size" \
    "$(jnum "$reg_ms")" "$(jnum "$jit_ms")" "$(jnum "$native_ms")" "$(jnum "$rust_ms")")")
done < "$cases_file"

{
  printf '{\n  "iterations": %s,\n  "warmup": %s,\n  "cases": [\n' "$iterations" "$warmup"
  for i in "${!json_rows[@]}"; do
    sep=","; [[ "$i" -eq $((${#json_rows[@]} - 1)) ]] && sep=""
    printf '    %s%s\n' "${json_rows[$i]}" "$sep"
  done
  printf '  ]\n}\n'
} > "$out"

printf '\nbaseline written to %s\n' "$out"
