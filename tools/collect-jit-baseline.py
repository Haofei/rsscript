#!/usr/bin/env python3
"""Collect a canonical native-JIT baseline on an explicitly controlled runner."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def output(command: list[str]) -> str:
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def cpu_name() -> str:
    if platform.system() == "Darwin":
        return output(["sysctl", "-n", "machdep.cpu.brand_string"])
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.exists():
        for line in cpuinfo.read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    return platform.processor() or "unknown"


def fixture_digest() -> str:
    digest = hashlib.sha256()
    for path in sorted((ROOT / "benchmarks/vm-jit").glob("**/*")):
        if path.is_file() and "baseline" not in path.parts:
            digest.update(path.relative_to(ROOT).as_posix().encode())
            digest.update(b"\0")
            digest.update(path.read_bytes())
            digest.update(b"\0")
    return digest.hexdigest()


def validate(document: dict) -> None:
    required = {
        "schema",
        "commit",
        "cpu",
        "os",
        "arch",
        "rust_version",
        "cranelift_version",
        "profile",
        "warmup",
        "samples",
        "fixture_digest",
        "evidence_class",
        "controlled",
        "cpu_affinity",
        "cpu_governor",
        "sample_order",
        "cases",
    }
    missing = sorted(required - document.keys())
    if missing:
        raise SystemExit(f"baseline is missing fields: {', '.join(missing)}")
    if document["schema"] != "rsscript.native_jit_baseline.v1":
        raise SystemExit("unexpected baseline schema")
    evidence_class = document["evidence_class"]
    if evidence_class not in {"controlled-canonical", "local-diagnostic"}:
        raise SystemExit("unexpected benchmark evidence class")
    expected_controlled = evidence_class == "controlled-canonical"
    if document["controlled"] is not expected_controlled:
        raise SystemExit("benchmark evidence class and controlled flag disagree")
    if document["samples"] < 20:
        raise SystemExit("benchmark evidence requires at least 20 samples")
    if expected_controlled:
        if document["cpu"].strip().lower() == "unknown":
            raise SystemExit("controlled baselines require a known CPU model")
        if document["cpu_affinity"].strip().lower() in {"", "none"}:
            raise SystemExit("controlled baselines require pinned CPU affinity")
        if document["cpu_governor"].strip().lower() in {"", "none", "unavailable"} or document[
            "cpu_governor"
        ].startswith("unavailable-"):
            raise SystemExit("controlled baselines require a known CPU governor")
    if document["sample_order"] != "alternating":
        raise SystemExit("canonical baselines require alternating sample order")
    if len(document["commit"]) != 40 or any(c not in "0123456789abcdef" for c in document["commit"]):
        raise SystemExit("commit must be a full lowercase SHA")
    case_evidence = {
        "case",
        "pass",
        "status",
        "interpreter_ns",
        "cold_e2e_native_ns",
        "interpreter_samples_ns",
        "cold_e2e_native_samples_ns",
        "interpreter_mad_ns",
        "cold_e2e_native_mad_ns",
        "warm_native_instrumented_ns",
        "speedup",
        "translation_nanos",
        "validation_nanos",
        "codegen_nanos",
        "finalize_nanos",
        "compile_nanos",
        "resident_code_bytes",
        "native_calls",
        "native_bails",
        "osr_entries",
        "continuation_entries",
        "runtime_helper_call_sites",
        "readonly_licm_sites",
        "bounds_check_sites",
        "bounds_checks_elided",
    }
    for case in document["cases"]:
        if case.get("controlled") is not expected_controlled:
            raise SystemExit(
                f"baseline case {case.get('case', '<unknown>')} controlled flag disagrees with evidence class"
            )
        missing_case = sorted(case_evidence - case.keys())
        if missing_case:
            raise SystemExit(
                f"baseline case {case.get('case', '<unknown>')} is missing evidence: "
                + ", ".join(missing_case)
            )
        for field in ("interpreter_samples_ns", "cold_e2e_native_samples_ns"):
            samples = case[field]
            if not isinstance(samples, list) or len(samples) != document["samples"]:
                raise SystemExit(
                    f"baseline case {case['case']} has {len(samples) if isinstance(samples, list) else 'invalid'} "
                    f"{field} samples; expected {document['samples']}"
                )
            if any(not isinstance(sample, int) or sample < 0 for sample in samples):
                raise SystemExit(
                    f"baseline case {case['case']} contains invalid {field} values"
                )
        if sorted(case["interpreter_samples_ns"])[document["samples"] // 2] != case["interpreter_ns"]:
            raise SystemExit(f"baseline case {case['case']} interpreter median does not match samples")
        if sorted(case["cold_e2e_native_samples_ns"])[document["samples"] // 2] != case["cold_e2e_native_ns"]:
            raise SystemExit(f"baseline case {case['case']} native median does not match samples")


def main() -> None:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--controlled", action="store_true")
    mode.add_argument("--local", action="store_true")
    parser.add_argument("--cpu-affinity")
    parser.add_argument("--cpu-governor")
    parser.add_argument("--samples", type=int, default=25)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.controlled and (not args.cpu_affinity or not args.cpu_governor):
        raise SystemExit("controlled collection requires --cpu-affinity and --cpu-governor")
    if args.samples < 20:
        raise SystemExit("canonical collection requires --samples >= 20")

    env = os.environ.copy()
    env["RSS_JIT_SAMPLES"] = str(args.samples)
    env["RSS_JIT_WARMUP"] = str(args.warmup)
    env["RSS_JIT_CONTROLLED"] = "1"
    command = [
        "cargo", "test", "--locked", "--release", "-p", "rsscript-sdk",
        "--features", "native-jit", "--test", "native_jit_scorecard",
        "--", "--ignored", "--nocapture",
    ]
    completed = subprocess.run(command, cwd=ROOT, env=env, text=True, capture_output=True, check=True)
    records = []
    header = None
    for line in (completed.stdout + "\n" + completed.stderr).splitlines():
        marker = "JIT_SCORECARD "
        if marker not in line:
            continue
        record = json.loads(line.split(marker, 1)[1])
        if "case" in record:
            if "interpreter_ns" in record and "cold_e2e_native_ns" in record:
                records.append(record)
        else:
            header = record
    if header is None or not records:
        raise SystemExit("scorecard did not emit a header and measured cases")

    controlled = bool(args.controlled)
    for record in records:
        record["controlled"] = controlled

    cranelift = output(["cargo", "tree", "-p", "rsscript-jit-cranelift", "-i", "cranelift-codegen"])
    document = {
        "schema": "rsscript.native_jit_baseline.v1",
        "commit": output(["git", "rev-parse", "HEAD"]),
        "cpu": cpu_name(),
        "os": platform.system().lower(),
        "arch": platform.machine().lower(),
        "rust_version": output(["rustc", "--version"]),
        "cranelift_version": cranelift.splitlines()[0],
        "profile": "release",
        "warmup": args.warmup,
        "samples": args.samples,
        "fixture_digest": fixture_digest(),
        "evidence_class": "controlled-canonical" if controlled else "local-diagnostic",
        "controlled": controlled,
        "cpu_affinity": args.cpu_affinity or "none",
        "cpu_governor": args.cpu_governor or "unavailable-local-host",
        "sample_order": header.get("order", "alternating"),
        "cases": records,
    }
    validate(document)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")
    print(args.output)


if __name__ == "__main__":
    main()
