#!/usr/bin/env python3
"""Summarise collected model samples into failure-class evidence.

Reads the `agent-eval` reports under `evals/samples/<model>/<mode>/` plus the
per-candidate `rss check --json` diagnostics, classifies each failing candidate,
and prints the counts the planning report is built from. Stdlib only.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EVALS = ROOT / "evals"
TASKS_DIR = EVALS / "tasks"

MODES = ("prompt_only", "language_card", "repair_loop")

# Ordered: the first matching rule wins for a given diagnostic.
CLASSES: list[tuple[str, str]] = [
    ("RS0206", "invents a symbol that is not in scope (host API or core name)"),
    ("RS0201", "omits argument labels at the call site"),
    ("RS0204", "omits argument labels at the call site"),
    ("RS0203", "invents an argument label that is not in the signature"),
    ("RS0205", "duplicate named argument"),
    ("RS0202", "wrong call-site data effect (missing/extra `mut` or `take`)"),
    ("RS0308", "wrong call-site data effect (missing/extra `mut` or `take`)"),
    ("RS0310", "wrong call-site data effect (missing/extra `mut` or `take`)"),
    ("RS0015", "hallucinated or unsupported syntax"),
    ("RS0013", "misuse of the `?` try operator"),
    ("RS0005", "redeclares interface symbols inside the implementation file"),
    ("RS0024", "invents a type that is not in scope"),
    ("RS0025", "invents a struct field"),
    ("RS0026", "references an unknown binding"),
    ("RS0027", "references an unknown protocol"),
    ("RS0028", "wrong receiver/`self` parameter convention"),
    ("RS0029", "`await` outside an async function"),
    ("RS0030", "awaits a non-async expression"),
    ("RS0031", "holds a local value across an `await`"),
    ("RS0032", "protocol not satisfied"),
    ("RS0021", "non-exhaustive match"),
    ("RS0037", "variant pattern arity mismatch"),
    ("RS0207", "argument type mismatch"),
    ("RS0208", "return type mismatch (often an implicit/missing `return`)"),
    ("RS0209", "control-flow type mismatch (branch used as a value)"),
    ("RS0210", "operator type mismatch"),
    ("RS0211", "derive requirement not satisfied"),
    ("RS0301", "managed-to-local conversion (`local` applied to a managed value)"),
    ("RS0302", "field/base partial-access conflict"),
    ("RS0303", "field/base partial-access conflict"),
    ("RS0304", "field/base partial-access conflict"),
    ("RS0305", "field/base partial-access conflict"),
    ("RS0306", "local binding of a class value"),
    ("RS0307", "invalid `manage` operand"),
    ("RS0311", "invalid assignment"),
    ("RS0312", "invalid assignment"),
    ("RS0313", "assignment type mismatch"),
    ("RS0401", "use after move (`take`n value used again)"),
    ("RS0411", "async function not lowerable"),
    ("RS0412", "`Task.cancellation_token()` outside a `task_group`"),
    ("RS0501", "missing `retains` for a parameter stored past return"),
    ("RS0007", "invalid `retains` clause"),
    ("RS0601", "`fresh` return is not clean"),
    ("RS0603", "invalid `fresh` return type"),
    ("RS0604", "`fresh` value requires a `local` binding"),
    ("RS0701", "resource used as an ordinary field"),
    ("RS0702", "resource escapes its `with` scope"),
    ("RS0704", "resource in an ordinary generic type"),
    ("RS0706", "missing `?` on a `Result` resource producer"),
    ("RS0801", "local captured by a managed closure"),
    ("RS0802", "`noescape` callback escapes"),
    ("RS0803", "local closure escapes"),
    ("RS0804", "`noescape` closure consumes a captured local"),
    ("RS0901", "`take` of a handle field"),
    ("RS0002", "missing return type"),
    ("RS0003", "missing parameter type"),
    ("RS0033", "integer literal out of range"),
    ("RS0034", "binding type cannot be inferred"),
    ("RS0040", "semantic analysis incomplete"),
    ("RS1001", "operator overload attempt"),
    ("RS1002", "implicit conversion attempt"),
]

CLASS_OF = dict(CLASSES)

# `RS0015 unsupported syntax` is by far the largest bucket, so it is worth
# splitting: the sub-classes below are what a language card would have to say
# something about. Matched against the offending source line.
SYNTAX_SUBCLASSES: list[tuple[str, re.Pattern[str]]] = [
    (
        "Rust/Swift struct literal `T { field: v }` instead of `T(field: v)`",
        re.compile(r"\b[A-Z][A-Za-z0-9_]*\s*\{\s*[a-z_][A-Za-z0-9_]*\s*:"),
    ),
    (
        "Rust path syntax `::` for namespaces or turbofish",
        re.compile(r"::"),
    ),
    (
        "expression-bodied match arm terminated by `,`",
        re.compile(r"=>\s*\S.*,\s*$"),
    ),
    (
        "`task_group`/`with`/`spawn` used as an expression or with a bare operand",
        re.compile(r"(task_group\s*[({])|(\bspawn\b)|(\bwith\s+[a-z_][A-Za-z0-9_]*\s*\{)"),
    ),
    (
        "invented `as` cast to a protocol/Dyn type",
        re.compile(r"\bas\s+Dyn<"),
    ),
    (
        "tuple destructuring in a `for` pattern",
        re.compile(r"for\s*\("),
    ),
    (
        "`mut` used as a binding keyword instead of `let mut`",
        re.compile(r"^\s*mut\s+[a-z_][A-Za-z0-9_]*\s*:"),
    ),
    (
        "`mut` written on the `with ... as` binding",
        re.compile(r"\bas\s+mut\b"),
    ),
]


def syntax_subclass(text: str) -> str:
    for label, pattern in SYNTAX_SUBCLASSES:
        if pattern.search(text or ""):
            return label
    return "other unsupported construct"


# `RS0206 unknown callee` is the second-largest bucket. The summary names the
# callee, so the split is exact rather than heuristic.
UNKNOWN_CALLEE = re.compile(r"call to `(.+?)` does not resolve")


def callee_subclass(summary: str) -> tuple[str, str]:
    match = UNKNOWN_CALLEE.search(summary or "")
    callee = match.group(1) if match else "?"
    bare = callee.replace("read ", "").replace("mut ", "").replace("take ", "")
    if bare in ("print", "write", "println", "log", "panic", "echo"):
        return "invents a bare output/print builtin", callee
    if "." in bare and bare[0].islower():
        return "receiver-method call on a value that has no such method", callee
    if "." in bare:
        return "invents a core-namespace function that does not exist", callee
    return "invents a free function that does not exist", callee


def load_tasks() -> dict[str, dict]:
    tasks = {}
    for path in sorted(TASKS_DIR.glob("*.toml")):
        with path.open("rb") as handle:
            task = tomllib.load(handle)
        tasks[task["id"]] = task
    return tasks


def run_check(source: Path, task: dict) -> list[dict]:
    command = [
        "cargo", "run", "-q", "-p", "rsscript-cli", "--bin", "rss", "--",
        "check", "--json", str(source),
    ]
    for relative in task.get("interfaces", []):
        command += ["--interface", str(EVALS / relative)]
    completed = subprocess.run(
        command, capture_output=True, text=True, cwd=ROOT, check=False
    )
    text = completed.stdout.strip()
    if not text:
        return []
    try:
        parsed = json.loads(text)
    except json.JSONDecodeError:
        return []
    return [d for d in parsed if d.get("severity") == "error"] if isinstance(parsed, list) else []


def analyze(model: str) -> dict:
    tasks = load_tasks()
    root = EVALS / "samples" / model
    out: dict = {"model": model, "modes": {}}
    for mode in MODES:
        mode_dir = root / mode
        report_path = mode_dir / "report.v1.json"
        if not report_path.is_file():
            continue
        report = json.loads(report_path.read_text())
        mode_data: dict = {
            "tasks": {},
            "aggregate": report["aggregate"],
            "missing_candidates": [],
        }
        for entry in report["tasks"]:
            task_id = entry["candidate"]["task_id"]
            source = mode_dir / task_id / "candidate.rss"
            if not source.is_file():
                mode_data["missing_candidates"].append(task_id)
                continue
            diagnostics = run_check(source, tasks[task_id])
            codes = sorted({d["code"] for d in diagnostics})
            classes = sorted({CLASS_OF.get(code, f"unclassified {code}") for code in codes})
            transcript_path = mode_dir / task_id / "transcript.json"
            transcript = (
                json.loads(transcript_path.read_text())
                if transcript_path.is_file()
                else []
            )
            mode_data["tasks"][task_id] = {
                "status": entry["status"],
                "static_check": entry["static_check"]["outcome"],
                "codes": codes,
                "classes": classes,
                "failed_invariants": [
                    f"{i['kind']}:{i['value']}"
                    for i in entry["invariants"]
                    if i["scope"] == "target" and not i["passed"]
                ],
                "repair_turns": entry["candidate"]["repair_turns"],
                "transcript": transcript,
                "examples": {
                    d["code"]: {
                        "summary": d.get("summary"),
                        "line": (d.get("primary_span") or {}).get("line"),
                        "text": (d.get("source_context") or {}).get("text", "").strip(),
                    }
                    for d in diagnostics
                },
                "syntax_subclasses": sorted(
                    {
                        syntax_subclass(
                            (d.get("source_context") or {}).get("text", "")
                        )
                        for d in diagnostics
                        if d["code"] == "RS0015"
                    }
                ),
                "callee_subclasses": sorted(
                    {
                        callee_subclass(d.get("summary", ""))[0]
                        for d in diagnostics
                        if d["code"] == "RS0206"
                    }
                ),
                "unknown_callees": sorted(
                    {
                        callee_subclass(d.get("summary", ""))[1]
                        for d in diagnostics
                        if d["code"] == "RS0206"
                    }
                ),
            }
        out["modes"][mode] = mode_data
    return out


def print_compare(data: dict) -> None:
    """Per-task and per-class comparison across the collected modes."""
    modes = [mode for mode in MODES if mode in data["modes"]]
    if not modes:
        return
    task_ids = sorted(data["modes"][modes[0]]["tasks"])

    print("\n## per-task outcome\n")
    print("| task | " + " | ".join(modes) + " |")
    print("|---|" + "---|" * len(modes))
    for task_id in task_ids:
        row = []
        for mode in modes:
            task = data["modes"][mode]["tasks"].get(task_id)
            if task is None:
                row.append("MISSING")
                continue
            mark = "pass" if task["status"] == "pass" else "fail"
            codes = ",".join(task["codes"]) or ("-" if mark == "pass" else "invariant")
            turns = f" t{task['repair_turns']}" if mode == "repair_loop" else ""
            row.append(f"{mark}{turns} ({codes})")
        print(f"| {task_id} | " + " | ".join(row) + " |")

    print("\n## class counts by mode\n")
    counters = {}
    for mode in modes:
        counter: Counter[str] = Counter()
        for task in data["modes"][mode]["tasks"].values():
            for cls in task["classes"]:
                counter[cls] += 1
        counters[mode] = counter
    all_classes = sorted(
        {cls for counter in counters.values() for cls in counter},
        key=lambda cls: -sum(counters[mode][cls] for mode in modes),
    )
    print("| class | " + " | ".join(modes) + " |")
    print("|---|" + "---|" * len(modes))
    for cls in all_classes:
        print(f"| {cls} | " + " | ".join(str(counters[m][cls]) for m in modes) + " |")

    if "repair_loop" in modes:
        print("\n## repair_loop turn outcomes\n")
        for task_id in task_ids:
            task = data["modes"]["repair_loop"]["tasks"].get(task_id)
            if task is None:
                continue
            steps = []
            for event in task["transcript"]:
                if event.get("check_ok"):
                    state = "ok"
                elif event.get("codes"):
                    state = ",".join(event["codes"])
                else:
                    state = event.get("error", "?")
                steps.append(f"turn{event.get('turn')}:{state}")
            print(f"{task_id}: {' -> '.join(steps)}")


def print_examples(data: dict, mode: str, only: str | None) -> None:
    """One concrete example per failure class, for the report's example column."""
    tasks = data["modes"].get(mode, {}).get("tasks", {})
    by_class: dict[str, list] = defaultdict(list)
    for task_id, task in tasks.items():
        for code, example in task["examples"].items():
            cls = CLASS_OF.get(code, f"unclassified {code}")
            if only and only not in cls and only != code:
                continue
            by_class[cls].append((task_id, code, example))
    for cls, rows in sorted(by_class.items(), key=lambda kv: -len(kv[1])):
        print(f"\n### {cls}  (n={len(rows)} candidates)")
        print("    codes:", dict(Counter(code for _, code, _ in rows)))
        for task_id, code, example in rows[:6]:
            print(
                f"    - {task_id} [{code}] line {example['line']}: {example['summary']}"
            )
            print(f"        > {example['text']}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default="sonnet")
    parser.add_argument("--out", default=None)
    parser.add_argument(
        "--reuse",
        action="store_true",
        help="read the existing analysis.json instead of re-running `rss check`",
    )
    parser.add_argument(
        "--compare", action="store_true", help="print the cross-mode comparison tables"
    )
    parser.add_argument(
        "--examples",
        metavar="MODE",
        help="print one concrete example per failure class for MODE",
    )
    parser.add_argument(
        "--only", help="with --examples, restrict to a class substring or a code"
    )
    args = parser.parse_args()
    target = (
        Path(args.out)
        if args.out
        else EVALS / "samples" / args.model / "analysis.json"
    )
    if args.reuse and target.is_file():
        data = json.loads(target.read_text())
    else:
        data = analyze(args.model)
        target.write_text(json.dumps(data, indent=2) + "\n")

    if args.examples:
        print_examples(data, args.examples, args.only)
        return 0
    if args.compare:
        print_compare(data)
        return 0

    for mode, mode_data in data["modes"].items():
        tasks = mode_data["tasks"]
        scored = sum(1 for t in tasks.values() if t["status"] == "pass")
        clean = sum(1 for t in tasks.values() if t["static_check"] == "pass")
        print(f"\n== {mode} (n={len(tasks)}) ==")
        print(f"  scorer pass : {scored}/{len(tasks)}")
        print(f"  compiles    : {clean}/{len(tasks)}")
        if mode_data["missing_candidates"]:
            print(f"  MISSING     : {mode_data['missing_candidates']}")
        counter: Counter[str] = Counter()
        for task in tasks.values():
            for cls in task["classes"]:
                counter[cls] += 1
        for cls, count in counter.most_common():
            print(f"    {count:3d}  {cls}")
        sub: Counter[str] = Counter()
        for task in tasks.values():
            for label in task.get("syntax_subclasses", []):
                sub[label] += 1
        if sub:
            print("  RS0015 sub-classes (candidates showing each):")
            for label, count in sub.most_common():
                print(f"    {count:3d}  {label}")
        callees: Counter[str] = Counter()
        names: Counter[str] = Counter()
        for task in tasks.values():
            for label in task.get("callee_subclasses", []):
                callees[label] += 1
            for name in task.get("unknown_callees", []):
                names[name] += 1
        if callees:
            print("  RS0206 sub-classes (candidates showing each):")
            for label, count in callees.most_common():
                print(f"    {count:3d}  {label}")
            print("  most-invented callees:")
            for name, count in names.most_common(12):
                print(f"    {count:3d}  {name}")
    print(f"\nwrote {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
