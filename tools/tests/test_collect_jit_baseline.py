#!/usr/bin/env python3

import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "collect-jit-baseline.py"
SPEC = importlib.util.spec_from_file_location("collect_jit_baseline", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def document():
    samples = list(range(100, 120))
    return {
        "schema": "rsscript.native_jit_baseline.v1",
        "commit": "a" * 40,
        "cpu": "fixed",
        "os": "linux",
        "arch": "x86_64",
        "rust_version": "rustc",
        "cranelift_version": "cranelift",
        "profile": "release",
        "warmup": 3,
        "samples": 20,
        "fixture_digest": "fixture",
        "controlled": True,
        "cpu_affinity": "0",
        "cpu_governor": "performance",
        "sample_order": "alternating",
        "cases": [
            {
                "case": "scalar",
                "pass": "baseline",
                "status": "entered",
                "interpreter_ns": 110,
                "cold_e2e_native_ns": 110,
                "interpreter_samples_ns": samples,
                "cold_e2e_native_samples_ns": samples,
                "interpreter_mad_ns": 5,
                "cold_e2e_native_mad_ns": 5,
                "warm_native_instrumented_ns": 80,
                "speedup": 1.0,
                "translation_nanos": 4,
                "validation_nanos": 4,
                "codegen_nanos": 8,
                "finalize_nanos": 4,
                "compile_nanos": 20,
                "resident_code_bytes": 40,
                "native_calls": 1,
                "native_bails": 0,
                "osr_entries": 0,
                "continuation_entries": 0,
                "runtime_helper_call_sites": 0,
                "readonly_licm_sites": 0,
                "bounds_check_sites": 0,
                "bounds_checks_elided": 0,
            }
        ],
    }


class CollectBaselineTests(unittest.TestCase):
    def test_accepts_complete_sample_distribution(self):
        MODULE.validate(document())

    def test_rejects_median_that_does_not_match_samples(self):
        invalid = document()
        invalid["cases"][0]["cold_e2e_native_ns"] = 999
        with self.assertRaises(SystemExit):
            MODULE.validate(invalid)

    def test_rejects_non_alternating_collection(self):
        invalid = document()
        invalid["sample_order"] = "native-first"
        with self.assertRaises(SystemExit):
            MODULE.validate(invalid)


if __name__ == "__main__":
    unittest.main()
