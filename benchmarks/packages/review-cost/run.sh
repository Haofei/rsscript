#!/usr/bin/env bash
# Review-cost benchmark: measures how RSScript review maps reduce review surface.
#
# Usage:
#   ./benchmarks/packages/review-cost/run.sh [--json]
#
# This script runs `rss review --map` on representative RSScript scenarios
# and reports review-cost reduction metrics.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

JSON_MODE=false
if [[ "${1:-}" == "--json" ]]; then
    JSON_MODE=true
fi

RSS_BIN="cargo run --quiet --bin rss --"

# Scenarios to benchmark (path, description)
declare -a SCENARIOS=(
    "tests/fixtures/pass/app-review-benchmark.rss|Full request handler (42 functions, multi-resource)"
    "tests/fixtures/pass/selfhost-review-classifier.rss|Self-hosted review classifier (complex logic)"
    "tests/fixtures/pass/selfhost-rustc-remap.rss|Rustc diagnostic remapping"
    "tests/fixtures/pass/selfhost-package-root-manifest.rss|Package manifest parsing"
    "tests/fixtures/pass/canonical-local-thumbnail.rss|Image thumbnail pipeline"
)

# Collect results
results=()

for scenario in "${SCENARIOS[@]}"; do
    IFS='|' read -r path description <<< "$scenario"

    if [[ ! -f "$path" ]]; then
        continue
    fi

    total_file_lines=$(wc -l < "$path" | tr -d ' ')

    # Run review map
    review_json=$($RSS_BIN review --json --map "$path" 2>/dev/null)

    total_lines=$(echo "$review_json" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['summary']['total_lines'])")
    must_review=$(echo "$review_json" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['summary']['must_review_lines'])")
    low_risk=$(echo "$review_json" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['summary']['low_semantic_risk_lines'])")
    unknown=$(echo "$review_json" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['summary']['unknown_lines'])")
    review_ratio=$(echo "$review_json" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['summary']['review_ratio'])")
    unknown_ratio=$(echo "$review_json" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['summary']['unknown_ratio'])")
    total_functions=$(echo "$review_json" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['summary']['total_functions'])")

    foldable_lines=$((total_lines - must_review - unknown))
    reduction_pct=$(python3 -c "print(f'{(1 - $review_ratio) * 100:.1f}')")

    results+=("$path|$description|$total_file_lines|$total_lines|$must_review|$foldable_lines|$unknown|$review_ratio|$unknown_ratio|$total_functions|$reduction_pct")
done

if [[ "$JSON_MODE" == "true" ]]; then
    # JSON output for CI consumption
    echo "{"
    echo "  \"benchmark\": \"review-cost\","
    echo "  \"version\": \"0.1\","
    echo "  \"scenarios\": ["
    first=true
    for result in "${results[@]}"; do
        IFS='|' read -r path desc file_lines sem_lines must_rev fold unk ratio unk_ratio funcs reduction <<< "$result"
        if [[ "$first" == "true" ]]; then
            first=false
        else
            echo ","
        fi
        cat <<EOF
    {
      "file": "$path",
      "description": "$desc",
      "file_lines": $file_lines,
      "semantic_lines": $sem_lines,
      "functions": $funcs,
      "must_review_lines": $must_rev,
      "foldable_lines": $fold,
      "unknown_lines": $unk,
      "review_ratio": $ratio,
      "unknown_ratio": $unk_ratio,
      "reduction_percent": $reduction
    }
EOF
    done
    echo ""
    echo "  ]"
    echo "}"
else
    # Human-readable table
    echo "═══════════════════════════════════════════════════════════════════════════════"
    echo "  RSScript Review-Cost Benchmark"
    echo "═══════════════════════════════════════════════════════════════════════════════"
    echo ""
    printf "%-50s %5s %5s %5s %5s %6s %6s\n" "Scenario" "Lines" "Must" "Fold" "Unk" "Ratio" "Saved"
    printf "%-50s %5s %5s %5s %5s %6s %6s\n" "──────────────────────────────────────────────────" "─────" "─────" "─────" "─────" "──────" "──────"

    total_must=0
    total_fold=0
    total_unk=0
    total_sem=0

    for result in "${results[@]}"; do
        IFS='|' read -r path desc file_lines sem_lines must_rev fold unk ratio unk_ratio funcs reduction <<< "$result"
        short_desc="${desc:0:50}"
        printf "%-50s %5s %5s %5s %5s %6s %5s%%\n" "$short_desc" "$sem_lines" "$must_rev" "$fold" "$unk" "$ratio" "$reduction"

        total_must=$((total_must + must_rev))
        total_fold=$((total_fold + fold))
        total_unk=$((total_unk + unk))
        total_sem=$((total_sem + sem_lines))
    done

    echo ""
    echo "───────────────────────────────────────────────────────────────────────────────"
    agg_ratio=$(python3 -c "print(f'{$total_must / $total_sem:.3f}') if $total_sem > 0 else print('0.000')")
    agg_reduction=$(python3 -c "print(f'{(1 - $total_must / $total_sem) * 100:.1f}') if $total_sem > 0 else print('0.0')")
    printf "%-50s %5s %5s %5s %5s %6s %5s%%\n" "TOTAL" "$total_sem" "$total_must" "$total_fold" "$total_unk" "$agg_ratio" "$agg_reduction"
    echo ""
    echo "Legend:"
    echo "  Lines  = total semantic lines (excl. blank/comments)"
    echo "  Must   = lines requiring human review (mutations, retentions, resources)"
    echo "  Fold   = lines a reviewer can skip (pure reads, struct definitions)"
    echo "  Unk    = lines with unknown risk (unresolved externals, native calls)"
    echo "  Ratio  = must_review / total (lower = less review needed)"
    echo "  Saved  = percentage of lines a reviewer can safely skip"
    echo ""
    echo "Interpretation:"
    echo "  - Raw diff of these files: $total_sem LOC to review"
    echo "  - With RSScript review map: $total_must LOC must-review + $total_fold foldable"
    echo "  - Review surface reduction: ${agg_reduction}% of lines can be folded"
    echo "  - Unknown/unresolved risk: $total_unk lines (must NOT be hidden)"
    echo ""
fi
