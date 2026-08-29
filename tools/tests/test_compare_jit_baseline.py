#!/usr/bin/env python3

import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "compare-jit-baseline.py"
SPEC = importlib.util.spec_from_file_location("compare_jit_baseline", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def baseline(runtime: int = 100, compile_ns: int = 50, code: int = 40):
    return {
        "schema": "rsscript.native_jit_baseline.v1",
        "controlled": True,
        "cpu": "fixed",
        "os": "linux",
        "arch": "x86_64",
        "rust_version": "rustc",
        "cranelift_version": "cranelift",
        "profile": "release",
        "fixture_digest": "fixture",
        "cases": [
            {
                "case": "scalar",
                "status": "entered",
                "semantic_match": True,
                "native_bails": 0,
                "retention_threshold_met": True,
                "cold_e2e_native_ns": runtime,
                "compile_nanos": compile_ns,
                "resident_code_bytes": code,
            }
        ],
    }


class CompareBaselineTests(unittest.TestCase):
    def compare(self, prior, current):
        return MODULE.compare(
            prior,
            current,
            runtime_regression_percent=10.0,
            compile_regression_percent=25.0,
            code_regression_percent=15.0,
        )

    def test_accepts_bounded_improvement(self):
        self.assertEqual(self.compare(baseline(), baseline(runtime=90)), [])

    def test_rejects_runtime_regression(self):
        failures = self.compare(baseline(), baseline(runtime=112))
        self.assertTrue(any("cold_e2e_native_ns" in failure for failure in failures))

    def test_rejects_lost_retention_or_new_bail(self):
        current = baseline()
        current["cases"][0]["retention_threshold_met"] = False
        current["cases"][0]["native_bails"] = 1
        failures = self.compare(baseline(), current)
        self.assertTrue(any("retention" in failure for failure in failures))
        self.assertTrue(any("bails" in failure for failure in failures))

    def test_rejects_machine_mismatch(self):
        current = baseline()
        current["cpu"] = "different"
        failures = self.compare(baseline(), current)
        self.assertTrue(any("environment mismatch" in failure for failure in failures))


if __name__ == "__main__":
    unittest.main()
