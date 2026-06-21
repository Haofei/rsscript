#!/usr/bin/env python3
"""Assemble one baseline JSON row from four per-mode bench blobs.

`run-baseline.sh` calls `rss bench --json` once per execution mode and hands the
raw blobs to this helper. We reshape that already-measured data into the row the
committed baseline stores. We do NOT recompute or re-measure any timing — we only
read the per-run `samples_ms` the bench binary now exposes and derive the
median / min / max / IQR (p25/p75) the §0.4 comparator needs.

Schema (per row):
  {
    "category": str, "case": str, "size": str,
    # legacy mean fields, KEPT unchanged for back-compat:
    "reg_vm_ms": <mean|null>, "jit_ms": ..., "native_ms": ..., "rust_ms": ...,
    # nested per-mode stats objects (null if the mode was unsupported/failed):
    "reg_vm": {"mean","median","min","max","p25","p75","samples"} | null,
    "jit": ... , "native": ... , "rust": ...
  }

Percentiles use linear interpolation between closest ranks (the same method as
numpy's default and the comparator), so p25/p75 are well-defined for any N>=1.
"""

import argparse
import json
import sys


def percentile(sorted_vals, q):
    """Linear-interpolation percentile of an ascending-sorted list. q in [0,100]."""
    if not sorted_vals:
        return None
    if len(sorted_vals) == 1:
        return sorted_vals[0]
    rank = (q / 100.0) * (len(sorted_vals) - 1)
    lo = int(rank)
    hi = min(lo + 1, len(sorted_vals) - 1)
    frac = rank - lo
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * frac


def median(sorted_vals):
    return percentile(sorted_vals, 50)


def mode_stats(blob):
    """Parse one bench blob into a stats object, or None if unsupported/failed.

    `blob` is the raw string captured from `rss bench --json`. An empty string or
    non-JSON value means the mode was unsupported/failed -> None (renders as JSON
    null, matching the legacy `n/a` cell).
    """
    blob = (blob or "").strip()
    if not blob.startswith("{"):
        return None
    try:
        data = json.loads(blob)
    except json.JSONDecodeError:
        return None

    samples = data.get("samples_ms")
    if isinstance(samples, list) and samples:
        vals = sorted(float(s) for s in samples)
        return {
            "mean": data.get("mean_ms", sum(vals) / len(vals)),
            "median": data.get("median_ms", median(vals)),
            "min": vals[0],
            "max": vals[-1],
            "p25": percentile(vals, 25),
            "p75": percentile(vals, 75),
            "samples": vals,
        }

    # Back-compat: an older bench binary emitted only min/mean/max with no
    # per-run samples. Surface what we have; without samples the spread bands
    # degrade gracefully (p25/p75 fall back to min/max) and the comparator's
    # noise band is conservative.
    mean = data.get("mean_ms")
    if mean is None:
        return None
    lo = data.get("min_ms", mean)
    hi = data.get("max_ms", mean)
    return {
        "mean": mean,
        "median": data.get("median_ms", mean),
        "min": lo,
        "max": hi,
        "p25": lo,
        "p75": hi,
        "samples": [],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--category", required=True)
    parser.add_argument("--case", required=True)
    parser.add_argument("--size", required=True)
    parser.add_argument("--reg", default="")
    parser.add_argument("--jit", default="")
    parser.add_argument("--native", default="")
    parser.add_argument("--rust", default="")
    args = parser.parse_args()

    modes = {
        "reg_vm": mode_stats(args.reg),
        "jit": mode_stats(args.jit),
        "native": mode_stats(args.native),
        "rust": mode_stats(args.rust),
    }

    row = {
        "category": args.category,
        "case": args.case,
        "size": args.size,
    }
    # Legacy mean fields (unchanged): *_ms == that mode's mean, or null.
    legacy_key = {"reg_vm": "reg_vm_ms", "jit": "jit_ms", "native": "native_ms", "rust": "rust_ms"}
    for mode, stats in modes.items():
        row[legacy_key[mode]] = stats["mean"] if stats else None
    # Nested per-mode stats objects.
    for mode, stats in modes.items():
        row[mode] = stats

    # Compact, stable separators so the row drops cleanly into the array.
    sys.stdout.write(json.dumps(row, separators=(",", ":")))


if __name__ == "__main__":
    main()
