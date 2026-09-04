#!/usr/bin/env python3

import importlib.util
import json
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "compare-jit-baseline.py"
SPEC = importlib.util.spec_from_file_location("compare_jit_baseline", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def baseline(runtime: int = 100, compile_ns: int = 50, code: int = 40):
    return {
        "schema": "rsscript.native_jit_baseline.v1",
        "evidence_class": "controlled-canonical",
        "controlled": True,
        "cpu": "fixed",
        "os": "linux",
        "arch": "x86_64",
        "rust_version": "rustc",
        "cranelift_version": "cranelift",
        "profile": "release",
        "fixture_digest": "fixture",
        "cpu_affinity": "0",
        "cpu_governor": "performance",
        "cases": [
            {
                "case": "scalar",
                "status": "entered",
                "semantic_match": True,
                "native_bails": 0,
                "retention_threshold_met": True,
                "cold_e2e_native_ns": runtime,
                "interpreter_samples_ns": [runtime * 2] * 20,
                "cold_e2e_native_samples_ns": [runtime] * 20,
                "interpreter_mad_ns": 0,
                "cold_e2e_native_mad_ns": 0,
                "warm_native_instrumented_ns": runtime,
                "translation_nanos": compile_ns // 5,
                "validation_nanos": compile_ns // 5,
                "codegen_nanos": compile_ns // 2,
                "finalize_nanos": compile_ns // 10,
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

    def test_loader_rejects_local_diagnostic_evidence(self):
        local = baseline()
        local["evidence_class"] = "local-diagnostic"
        local["controlled"] = False
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "local.json"
            path.write_text(json.dumps(local))
            with self.assertRaises(ValueError):
                MODULE.load(path)


if __name__ == "__main__":
    unittest.main()
