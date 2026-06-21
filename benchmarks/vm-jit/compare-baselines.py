#!/usr/bin/env python3
"""Noise-aware baseline comparator for the VM/JIT perf work (plan §0.4).

Compares two baseline JSON files (REF = "before", CUR = "after") produced by
`run-baseline.sh`, per kernel and per execution mode, and decides whether any
kernel *regressed* under the §0.4 rule:

    A kernel regresses in a mode iff
        delta% > threshold (default 10%)   AND   delta% > spread-band%
    where
        delta%      = (cur - ref) / ref * 100        (compared on the MEDIAN)
        spread-band = the CURRENT run's relative spread, max of
                        (max - min) / median * 100  and  IQR/median * 100
    Improvements (negative delta) are reported but never fail.

If a mode's median is absent (old baseline schema), we fall back to its mean
(`*_ms`). If spread fields are absent we treat the spread band as 0, so the rule
collapses to the bare ">threshold" check — conservative, never hides a regression.

Exit code: non-zero if any REGRESSION in the requested mode(s)/cohort, else zero.
This wires it as a CI/PR gate. `--mode native` focuses the Phase-3.0 criterion
(native-bail kernels must not get slower under jit-native).

Usage:
    compare-baselines.py REF.json CUR.json
        [--threshold-pct 10] [--mode reg_vm|jit|native|rust|all]
        [--cohort CATEGORY] [--json]
"""

import argparse
import json
import sys

MODES = ["reg_vm", "jit", "native", "rust"]
LEGACY_MEAN = {"reg_vm": "reg_vm_ms", "jit": "jit_ms", "native": "native_ms", "rust": "rust_ms"}


def kernel_key(row):
    """Stable identity for a kernel across files: (case, size)."""
    return (row.get("case"), str(row.get("size")))


def load(path):
    with open(path) as handle:
        doc = json.load(handle)
    return {kernel_key(row): row for row in doc.get("cases", [])}


def mode_center(row, mode):
    """The comparison statistic for a mode: median if present, else mean.

    Returns None if the mode is unsupported/absent (renders as `n/a`).
    """
    nested = row.get(mode)
    if isinstance(nested, dict):
        if nested.get("median") is not None:
            return nested["median"]
        if nested.get("mean") is not None:
            return nested["mean"]
    legacy = row.get(LEGACY_MEAN[mode])
    if legacy is not None:
        return legacy
    return None


def mode_spread_pct(row, mode, center):
    """Relative spread band (percent) for a mode from the CURRENT run.

    max of range-based ((max-min)/median) and IQR-based ((p75-p25)/median),
    each in percent. Absent spread fields -> 0.0 (conservative).
    """
    nested = row.get(mode)
    if not isinstance(nested, dict) or not center:
        return 0.0
    band = 0.0
    lo, hi = nested.get("min"), nested.get("max")
    if lo is not None and hi is not None:
        band = max(band, (hi - lo) / center * 100.0)
    p25, p75 = nested.get("p25"), nested.get("p75")
    if p25 is not None and p75 is not None:
        band = max(band, (p75 - p25) / center * 100.0)
    return band


def compare(ref, cur, modes, cohort, threshold):
    results = []
    for key in sorted(cur.keys() & ref.keys()):
        cur_row, ref_row = cur[key], ref[key]
        category = cur_row.get("category")
        if cohort is not None and category != cohort:
            continue
        for mode in modes:
            ref_c = mode_center(ref_row, mode)
            cur_c = mode_center(cur_row, mode)
            if ref_c is None or cur_c is None:
                results.append({
                    "category": category, "case": cur_row.get("case"),
                    "size": str(cur_row.get("size")), "mode": mode,
                    "ref": ref_c, "cur": cur_c, "delta_pct": None,
                    "spread_pct": None, "verdict": "n/a",
                })
                continue
            if ref_c == 0:
                delta = float("inf") if cur_c > 0 else 0.0
            else:
                delta = (cur_c - ref_c) / ref_c * 100.0
            spread = mode_spread_pct(cur_row, mode, cur_c)
            if delta < 0:
                verdict = "improved"
            elif delta <= threshold:
                verdict = "OK"
            elif delta <= spread:
                verdict = "within-noise"
            else:
                verdict = "REGRESSION"
            results.append({
                "category": category, "case": cur_row.get("case"),
                "size": str(cur_row.get("size")), "mode": mode,
                "ref": ref_c, "cur": cur_c, "delta_pct": delta,
                "spread_pct": spread, "verdict": verdict,
            })
    return results


def fmt_num(value):
    if value is None:
        return "n/a"
    if value == float("inf"):
        return "+inf"
    return f"{value:.3f}"


def fmt_pct(value):
    if value is None:
        return "n/a"
    if value == float("inf"):
        return "+inf"
    return f"{value:+.1f}%"


def print_table(results, threshold):
    by_cohort = {}
    for entry in results:
        by_cohort.setdefault(entry["category"], []).append(entry)

    overall = {"REGRESSION": 0, "improved": 0, "within-noise": 0, "OK": 0, "n/a": 0}
    header = f"{'kernel':<32} {'mode':<8} {'ref':>10} {'cur':>10} {'delta':>9} {'spread':>9}  verdict"
    for cohort in sorted(by_cohort):
        entries = by_cohort[cohort]
        print(f"\n== cohort: {cohort} ==")
        print(header)
        print("-" * len(header))
        counts = {"REGRESSION": 0, "improved": 0, "within-noise": 0, "OK": 0, "n/a": 0}
        for e in entries:
            counts[e["verdict"]] += 1
            overall[e["verdict"]] += 1
            print(f"{e['case']:<32} {e['mode']:<8} {fmt_num(e['ref']):>10} "
                  f"{fmt_num(e['cur']):>10} {fmt_pct(e['delta_pct']):>9} "
                  f"{fmt_pct(e['spread_pct']):>9}  {e['verdict']}")
        print(f"  cohort summary: {counts['REGRESSION']} regression(s), "
              f"{counts['improved']} improved, {counts['within-noise']} within-noise, "
              f"{counts['OK']} ok, {counts['n/a']} n/a")

    print(f"\n== overall (threshold {threshold:.0f}%) ==")
    print(f"  {overall['REGRESSION']} regression(s), {overall['improved']} improved, "
          f"{overall['within-noise']} within-noise, {overall['OK']} ok, {overall['n/a']} n/a")
    return overall["REGRESSION"]


def main():
    parser = argparse.ArgumentParser(description="Noise-aware VM/JIT baseline comparator (plan §0.4).")
    parser.add_argument("ref", help="reference (before) baseline JSON")
    parser.add_argument("cur", help="current (after) baseline JSON")
    parser.add_argument("--threshold-pct", type=float, default=10.0)
    parser.add_argument("--mode", choices=MODES + ["all"], default="all")
    parser.add_argument("--cohort", default=None, help="restrict to one category")
    parser.add_argument("--json", action="store_true", help="emit machine-readable results")
    args = parser.parse_args()

    ref = load(args.ref)
    cur = load(args.cur)
    modes = MODES if args.mode == "all" else [args.mode]

    results = compare(ref, cur, modes, args.cohort, args.threshold_pct)
    regressions = sum(1 for e in results if e["verdict"] == "REGRESSION")

    if args.json:
        print(json.dumps({
            "threshold_pct": args.threshold_pct,
            "mode": args.mode,
            "cohort": args.cohort,
            "regressions": regressions,
            "results": results,
        }, indent=2))
    else:
        print_table(results, args.threshold_pct)

    return 1 if regressions else 0


if __name__ == "__main__":
    sys.exit(main())
