#!/usr/bin/env python3
"""Compare two native-JIT baselines produced on the same controlled host."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import Any


ENVIRONMENT_FIELDS = (
    "cpu",
    "os",
    "arch",
    "rust_version",
    "cranelift_version",
    "profile",
    "fixture_digest",
)


def load(path: pathlib.Path) -> dict[str, Any]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema") != "rsscript.native_jit_baseline.v1":
        raise ValueError(f"{path}: unsupported baseline schema")
    if document.get("controlled") is not True:
        raise ValueError(f"{path}: baseline is not controlled evidence")
    return document


def allowed(current: int | float, prior: int | float, percent: float) -> bool:
    if prior == 0:
        return current == 0
    return current <= prior * (1.0 + percent / 100.0)


def compare(
    prior: dict[str, Any],
    current: dict[str, Any],
    *,
    runtime_regression_percent: float,
    compile_regression_percent: float,
    code_regression_percent: float,
) -> list[str]:
    failures: list[str] = []
    for field in ENVIRONMENT_FIELDS:
        if prior.get(field) != current.get(field):
            failures.append(
                f"environment mismatch for {field}: "
                f"{prior.get(field)!r} != {current.get(field)!r}"
            )

    prior_cases = {case["case"]: case for case in prior.get("cases", [])}
    current_cases = {case["case"]: case for case in current.get("cases", [])}
    for name, before in prior_cases.items():
        after = current_cases.get(name)
        if after is None:
            failures.append(f"missing canonical case: {name}")
            continue
        if after.get("semantic_match") is not True:
            failures.append(f"{name}: semantic differential failed")
        if before.get("status") == "entered" and after.get("status") != "entered":
            failures.append(f"{name}: native execution no longer enters")
        if after.get("native_bails", 0) > before.get("native_bails", 0):
            failures.append(
                f"{name}: native bails increased "
                f"{before.get('native_bails', 0)} -> {after.get('native_bails', 0)}"
            )
        if before.get("retention_threshold_met") is True and after.get(
            "retention_threshold_met"
        ) is not True:
            failures.append(f"{name}: lost the controlled retention threshold")

        metrics = (
            (
                "cold_e2e_native_ns",
                runtime_regression_percent,
            ),
            ("warm_native_instrumented_ns", runtime_regression_percent),
            ("translation_nanos", compile_regression_percent),
            ("validation_nanos", compile_regression_percent),
            ("codegen_nanos", compile_regression_percent),
            ("finalize_nanos", compile_regression_percent),
            ("compile_nanos", compile_regression_percent),
            ("resident_code_bytes", code_regression_percent),
        )
        for metric, threshold in metrics:
            before_value = before.get(metric)
            after_value = after.get(metric)
            if not isinstance(before_value, (int, float)) or not isinstance(
                after_value, (int, float)
            ):
                failures.append(f"{name}: missing numeric metric {metric}")
                continue
            if not allowed(after_value, before_value, threshold):
                failures.append(
                    f"{name}: {metric} regressed {before_value} -> {after_value} "
                    f"(allowed +{threshold:.1f}%)"
                )

    extra = sorted(current_cases.keys() - prior_cases.keys())
    if extra:
        print("new canonical cases (not regression-gated yet): " + ", ".join(extra))
    return failures


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("prior", type=pathlib.Path)
    parser.add_argument("current", type=pathlib.Path)
    parser.add_argument("--runtime-regression-percent", type=float, default=10.0)
    parser.add_argument("--compile-regression-percent", type=float, default=25.0)
    parser.add_argument("--code-regression-percent", type=float, default=15.0)
    args = parser.parse_args()

    try:
        failures = compare(
            load(args.prior),
            load(args.current),
            runtime_regression_percent=args.runtime_regression_percent,
            compile_regression_percent=args.compile_regression_percent,
            code_regression_percent=args.code_regression_percent,
        )
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"baseline comparison failed: {error}", file=sys.stderr)
        return 2
    if failures:
        for failure in failures:
            print(f"regression: {failure}", file=sys.stderr)
        return 1
    print("controlled native-JIT baseline comparison passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
